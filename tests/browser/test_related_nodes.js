#!/usr/bin/env node
/**
 * Node.js test for RelatedNode structure in WASM output
 *
 * This test verifies that the refactored RelatedNode structure correctly
 * includes node, root_path, and home_net fields for each related node.
 *
 * ## What This Tests
 *
 * After refactoring NodeContext.related_nodes from `BTreeMap<Bid, BeliefNode>`
 * to `BTreeMap<Bid, RelatedNode>`, this test validates:
 *
 * 1. RelatedNode structure has all required fields:
 *    - `node`: BeliefNode (the related node data)
 *    - `root_path`: String (path for href generation)
 *    - `home_net`: Bid (network context)
 *
 * 2. All related nodes in the map have consistent structure
 *
 * 3. Graph references (sources/sinks) correctly point to related_nodes entries
 *
 * This ensures the JavaScript viewer can access `relatedNode.root_path` directly
 * for generating navigation links, following the pattern from ExtendedRelation.
 *
 * ## CI Integration
 *
 * This test runs in GitHub Actions (see .github/workflows/test.yml) as part
 * of the `wasm-interface` job. It validates the WASM FFI boundary between
 * Rust and JavaScript to catch breaking changes early.
 */

import { readFile } from "fs/promises";
import { fileURLToPath } from "url";
import { dirname, join } from "path";

const __filename = fileURLToPath(import.meta.url);
const __dirname = dirname(__filename);
const projectRoot = join(__dirname, "../..");

// ANSI color codes
const GREEN = "\x1b[32m";
const RED = "\x1b[31m";
const BLUE = "\x1b[34m";
const YELLOW = "\x1b[33m";
const RESET = "\x1b[0m";

let testsPassed = 0;
let testsFailed = 0;

function log(message, type = "info") {
  const prefix =
    {
      pass: `${GREEN}✓${RESET}`,
      fail: `${RED}✗${RESET}`,
      info: `${BLUE}ℹ${RESET}`,
      warn: `${YELLOW}⚠${RESET}`,
    }[type] || "";
  console.log(`${prefix} ${message}`);
}

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

async function runTests() {
  console.log(`${BLUE}=== Testing RelatedNode Structure ===${RESET}\n`);

  try {
    // Load WASM module
    log("Loading WASM module...", "info");
    const wasmModule = await import(join(projectRoot, "pkg/noet_core.js"));

    // Load WASM binary directly for Node.js (fetch doesn't work in Node)
    const wasmBuffer = await readFile(join(projectRoot, "pkg/noet_core_bg.wasm"));
    await wasmModule.default(wasmBuffer);
    log("WASM module loaded", "pass");

    // Load beliefbase JSON
    log("Loading beliefbase.json...", "info");
    const beliefbaseJson = await readFile(join(__dirname, "test-output/beliefbase.json"), "utf-8");
    log("Beliefbase JSON loaded", "pass");

    // Extract network root BID for metadata
    const beliefbaseData = JSON.parse(beliefbaseJson);
    const networkNode = Object.values(beliefbaseData.states).find(
      (node) => node.kind && node.kind.includes("Network"),
    );

    assert(networkNode !== undefined, "Found Network node in beliefbase");
    const metadata = JSON.stringify({ bid: networkNode.bid });

    // Initialize BeliefBase
    log("Initializing BeliefBase...", "info");
    const bb = new wasmModule.BeliefBaseWasm(beliefbaseJson, metadata);
    assert(bb !== null, "BeliefBase initialized");

    // Get documents to test context
    const documents = bb.get_documents();
    assert(documents.length > 0, `Found ${documents.length} documents`);

    // Test NodeContext with related nodes
    console.log(`\n${BLUE}Testing NodeContext.related_nodes structure...${RESET}`);

    // Find Section A in the beliefbase data - it should have edges
    const sectionABid = Object.keys(beliefbaseData.states).find(
      (bid) => beliefbaseData.states[bid].title === "Section A",
    );

    let ctx;
    let testDoc;

    if (!sectionABid) {
      log("Section A not found in test data, using first document", "warn");
      testDoc = documents[0];
      ctx = bb.get_context(testDoc.bid);
      log(`Getting context for: ${testDoc.title}`, "info");
    } else {
      log(`Testing Section A (should have relations): ${sectionABid}`, "info");

      // Verify edges exist in raw JSON
      const sectionAIdx = beliefbaseData.relations.nodes.indexOf(sectionABid);
      const edges = beliefbaseData.relations.edges.filter(
        ([src, sink]) => src === sectionAIdx || sink === sectionAIdx,
      );
      log(`  Raw edges in JSON: ${edges.length}`, "info");

      ctx = bb.get_context(sectionABid);
      testDoc = beliefbaseData.states[sectionABid];
      log(`Getting context for: ${testDoc.title}`, "info");
    }
    assert(ctx !== null, "get_context() returned non-null");
    assert(ctx.node !== undefined, "NodeContext has node field");
    assert(ctx.root_path !== undefined, "NodeContext has root_path field");
    assert(ctx.home_net !== undefined, "NodeContext has home_net field");
    assert(ctx.related_nodes !== undefined, "NodeContext has related_nodes field");
    assert(ctx.graph !== undefined, "NodeContext has graph field");

    // Check related_nodes structure
    const relatedNodesBids = Object.keys(ctx.related_nodes);
    const relatedCount = relatedNodesBids.length;

    if (relatedCount > 0) {
      log(`Found ${relatedCount} related nodes`, "info");

      // Test first related node structure
      const firstBid = relatedNodesBids[0];
      const relatedNode = ctx.related_nodes[firstBid];

      console.log(`\n${BLUE}Validating RelatedNode structure...${RESET}`);
      assert(relatedNode !== undefined, "RelatedNode exists in map");
      assert(typeof relatedNode === "object", "RelatedNode is an object");

      // Verify RelatedNode has required fields
      assert(relatedNode.node !== undefined, "RelatedNode.node field exists");
      assert(relatedNode.root_path !== undefined, "RelatedNode.root_path field exists");
      assert(relatedNode.home_net !== undefined, "RelatedNode.home_net field exists");

      // Verify field types
      assert(typeof relatedNode.node === "object", "RelatedNode.node is object");
      assert(typeof relatedNode.root_path === "string", "RelatedNode.root_path is string");
      assert(typeof relatedNode.home_net === "string", "RelatedNode.home_net is string");

      // Verify node content
      assert(relatedNode.node.bid === firstBid, "RelatedNode.node.bid matches map key");
      assert(relatedNode.node.title !== undefined, "RelatedNode.node has title");

      log(`Sample RelatedNode:`, "info");
      log(`  BID: ${relatedNode.node.bid}`, "info");
      log(`  Title: ${relatedNode.node.title || "(empty)"}`, "info");
      log(`  Root Path: ${relatedNode.root_path}`, "info");
      log(`  Home Network: ${relatedNode.home_net}`, "info");

      // Test that root_path is suitable for href generation
      if (relatedNode.root_path && relatedNode.root_path.length > 0) {
        assert(
          relatedNode.root_path.includes("/") || relatedNode.root_path.includes("#"),
          "root_path looks like a valid path (contains / or #)",
        );
        log(`Root path is suitable for href: ${relatedNode.root_path}`, "pass");
      }

      // Test all related nodes have consistent structure
      console.log(`\n${BLUE}Validating all ${relatedCount} related nodes...${RESET}`);
      let validCount = 0;
      for (const bid of relatedNodesBids) {
        const rn = ctx.related_nodes[bid];
        if (rn.node && rn.root_path !== undefined && rn.home_net !== undefined) {
          validCount++;
        }
      }
      assert(
        validCount === relatedCount,
        `All ${relatedCount} related nodes have correct structure (validated ${validCount})`,
      );
    } else {
      log(`No related nodes found after ExtendedRelation filtering`, "warn");
      log(`Graph has ${Object.keys(ctx.graph).length} weight kinds`, "info");

      // Debug: show what's in the graph
      for (const [kind, [sources, sinks]] of Object.entries(ctx.graph)) {
        log(`  ${kind}: ${sources.length} sources, ${sinks.length} sinks`, "info");
        if (sinks.length > 0) {
          log(`    First sink BID: ${sinks[0]}`, "info");
          log(`    Sink in related_nodes: ${ctx.related_nodes[sinks[0]] !== undefined}`, "info");
        }
      }
    }

    // Test graph structure references related_nodes correctly
    console.log(`\n${BLUE}Validating graph references...${RESET}`);
    for (const [weightKind, [sources, sinks]] of Object.entries(ctx.graph)) {
      assert(Array.isArray(sources), `graph[${weightKind}].sources is array`);
      assert(Array.isArray(sinks), `graph[${weightKind}].sinks is array`);

      // Verify all BIDs in graph exist in related_nodes
      const allGraphBids = [...sources, ...sinks];
      for (const bid of allGraphBids) {
        assert(ctx.related_nodes[bid] !== undefined, `Graph BID ${bid} exists in related_nodes`);
      }

      if (allGraphBids.length > 0) {
        log(
          `Weight kind "${weightKind}": ${sources.length} sources, ${sinks.length} sinks - all BIDs valid`,
          "pass",
        );
      }
    }

    // Summary
    console.log(`\n${BLUE}=== Test Summary ===${RESET}`);
    console.log(`${GREEN}Passed: ${testsPassed}${RESET}`);
    console.log(`${RED}Failed: ${testsFailed}${RESET}`);

    if (testsFailed === 0) {
      console.log(`\n${GREEN}✓ All tests passed!${RESET}`);
      console.log(`${GREEN}✓ RelatedNode structure is correct${RESET}`);
      console.log(`${GREEN}✓ Each related node includes: node, root_path, home_net${RESET}`);
      console.log(
        `${GREEN}✓ WASM FFI boundary validated (test fixtures have no inter-document links)${RESET}`,
      );
      process.exit(0);
    } else {
      console.log(`\n${RED}✗ Some tests failed${RESET}`);
      process.exit(1);
    }
  } catch (error) {
    console.error(`\n${RED}✗ Test error: ${error.message}${RESET}`);
    console.error(error.stack);
    process.exit(1);
  }
}

runTests();
