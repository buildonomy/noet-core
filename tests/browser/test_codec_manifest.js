#!/usr/bin/env node
/**
 * Node.js test for WASM codec manifest loading
 *
 * ## What This Tests
 *
 * The WASM viewer's extension classification depends on knowing which file
 * extensions produce rendered HTML pages.  On native builds this is determined
 * at runtime by `CODECS` and `WALK_CODECS`, but those registries are absent
 * on `wasm32`.  The codec manifest (`codecs.json`) bridges this gap:
 *
 *   1. `export_beliefbase` writes `codecs.json` alongside the beliefbase data.
 *   2. The WASM viewer fetches it and calls `setKnownExtensions(json)`.
 *   3. `normalize_path_extension` and `is_known_codec_extension` then correctly
 *      rewrite custom extensions (e.g. `.yaml`) to `.html`.
 *
 * This test validates the full round-trip:
 *   - `codecs.json` is written during test data generation
 *   - `setKnownExtensions` parses it without error
 *   - `normalizePathExtension` rewrites built-in extensions (`.md`, `.xlsx`)
 *   - `normalizePathExtension` rewrites walk-tracked extensions (`.yaml`)
 *   - Extensions no codec claims are treated as dotted directory names
 *   - Graceful handling of missing/malformed manifests
 *
 * ## CI Integration
 *
 * Runs in the `wasm-interface` GitHub Actions job alongside test_nav_tree.js
 * and test_related_nodes.js.  Requires:
 *   - WASM module built:  `cargo build --features bin`
 *   - Test data generated: `noet parse tests/network_1 --html-output tests/browser/test-output`
 */

import { readFile, readdir } from "fs/promises";
import { readFileSync, existsSync } from "fs";
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

/** Like assert but doesn't throw — records failure and continues. */
function check(condition, message) {
  if (condition) {
    testsPassed++;
    log(message, "pass");
  } else {
    testsFailed++;
    log(message, "fail");
  }
}

// ── Main test suite ──────────────────────────────────────────────────────────

async function runTests() {
  console.log(`${BLUE}=== Testing Codec Manifest (codecs.json → WASM) ===${RESET}\n`);

  // ── 1. Load WASM module ─────────────────────────────────────────────────
  console.log(`${BLUE}[1/5] Loading WASM module${RESET}`);

  const wasmModule = await import(
    join(projectRoot, "target/wasm-build/pkg/noet_core.js")
  );
  const wasmBuffer = await readFile(
    join(projectRoot, "target/wasm-build/pkg/noet_core_bg.wasm"),
  );
  await wasmModule.default(wasmBuffer);
  assert(true, "WASM module loaded");

  const { BeliefBaseWasm } = wasmModule;

  // ── 2. Verify codecs.json was written ───────────────────────────────────
  console.log(`\n${BLUE}[2/5] Verifying codecs.json exists${RESET}`);

  const codecsPath = join(__dirname, "test-output/codecs.json");
  assert(existsSync(codecsPath), "codecs.json exists in test-output/");

  const codecsJson = readFileSync(codecsPath, "utf-8");
  const codecsManifest = JSON.parse(codecsJson);
  assert(codecsManifest.version === "1", `codecs.json version is "1"`);
  assert(
    Array.isArray(codecsManifest.document_extensions),
    "codecs.json has document_extensions array",
  );
  assert(
    codecsManifest.document_extensions.length > 0,
    `document_extensions has ${codecsManifest.document_extensions.length} entries`,
  );

  log(
    `Extensions in manifest: [${codecsManifest.document_extensions.join(", ")}]`,
    "info",
  );

  // Verify expected extensions are present
  check(codecsManifest.document_extensions.includes("md"), 'manifest includes "md"');
  check(codecsManifest.document_extensions.includes("xlsx"), 'manifest includes "xlsx"');
  check(codecsManifest.document_extensions.includes("ods"), 'manifest includes "ods"');
  check(
    codecsManifest.document_extensions.includes("yaml"),
    'manifest includes "yaml" (from YamlWalkCodec)',
  );
  check(
    codecsManifest.document_extensions.includes("yml"),
    'manifest includes "yml" (from YamlWalkCodec)',
  );

  // Verify no empty strings leaked through
  check(
    !codecsManifest.document_extensions.includes(""),
    "manifest does not include empty string",
  );

  // ── 3. Test normalize_path_extension BEFORE loading manifest ────────────
  console.log(
    `\n${BLUE}[3/5] Testing normalizePathExtension (before setKnownExtensions)${RESET}`,
  );

  // Built-in extensions should work even without the manifest
  check(
    BeliefBaseWasm.normalize_path_extension("docs/guide.md") === "docs/guide.html",
    ".md → .html (built-in, before manifest)",
  );
  check(
    BeliefBaseWasm.normalize_path_extension("data/items.xlsx") === "data/items.html",
    ".xlsx → .html (built-in, before manifest)",
  );
  check(
    BeliefBaseWasm.normalize_path_extension("data/items.ods") === "data/items.html",
    ".ods → .html (built-in, before manifest)",
  );

  // `.yaml` is a document extension on native (YamlWalkCodec is registered
  // unconditionally in WALK_CODECS), so "config/data.yaml" compiles to
  // "config/data.html" — which step 4 asserts once the manifest is loaded.
  //
  // Before the manifest, the WASM side has no way to know that: it sees only
  // BUILTIN_EXTENSIONS. So it falls through to Case 4 and treats the path as a
  // dotted directory. That output is *wrong*, not a contract — it is the
  // degraded answer this whole manifest mechanism exists to prevent. The viewer
  // never ships it: initializeWasm() now throws if codecs.json is missing or
  // malformed, so this state exists only here, where the test drives the
  // wasm-bindgen module directly and controls when the manifest is applied.
  //
  // Assert the invariant, not the degraded string: a shim/walk-codec extension
  // is not yet resolvable. Pinning the exact fallback shape here would bless
  // the wrong answer and make a future improvement look like a regression.
  const yamlBeforeManifest = BeliefBaseWasm.normalize_path_extension("config/data.yaml");
  check(
    yamlBeforeManifest !== "config/data.html",
    ".yaml not yet resolvable before manifest (degraded, not a contract)",
  );
  log(`  └─ degraded fallback was "${yamlBeforeManifest}"`, "info");

  // An extension that no codec claims is a dotted *directory* name, not a file
  // — see "Case 4" in normalize_path_extension_impl. Corpora use dotted
  // directories (e.g. a "widget.media/" sibling holding a page's images), and
  // AnchorPath cannot distinguish those from a file with an exotic extension.
  //
  // Unlike the .yaml case above, this IS the contract: no codec claims
  // ".media", before or after the manifest, so the directory reading is the
  // only one available. Genuine assets never reach this function — callers
  // guard against asset-namespace BIDs before normalising.
  check(
    BeliefBaseWasm.normalize_path_extension("catalog/widget.media") ===
      "catalog/widget.media/index.html",
    "dotted directory → /index.html (before manifest)",
  );

  // ── 4. Load codec manifest and test extension rewriting ─────────────────
  console.log(`\n${BLUE}[4/5] Loading codec manifest via setKnownExtensions${RESET}`);

  // This is the critical call that bridges native extensions to WASM
  BeliefBaseWasm.setKnownExtensions(codecsJson);
  assert(true, "setKnownExtensions() succeeded");

  // Now .yaml and .yml should be rewritten to .html
  check(
    BeliefBaseWasm.normalize_path_extension("config/data.yaml") === "config/data.html",
    ".yaml → .html (after manifest)",
  );
  check(
    BeliefBaseWasm.normalize_path_extension("config/data.yml") === "config/data.html",
    ".yml → .html (after manifest)",
  );

  // Built-in extensions should still work
  check(
    BeliefBaseWasm.normalize_path_extension("docs/guide.md") === "docs/guide.html",
    ".md → .html (after manifest)",
  );
  check(
    BeliefBaseWasm.normalize_path_extension("data/items.xlsx") === "data/items.html",
    ".xlsx → .html (after manifest)",
  );

  // Anchors should be preserved
  check(
    BeliefBaseWasm.normalize_path_extension("config/data.yaml#section") ===
      "config/data.html#section",
    ".yaml#anchor → .html#anchor (after manifest)",
  );
  check(
    BeliefBaseWasm.normalize_path_extension("docs/guide.md#intro") ===
      "docs/guide.html#intro",
    ".md#anchor → .html#anchor (after manifest)",
  );

  // Directory paths should still append /index.html
  check(
    BeliefBaseWasm.normalize_path_extension("mynetwork") === "mynetwork/index.html",
    "directory path → /index.html (after manifest)",
  );
  check(
    BeliefBaseWasm.normalize_path_extension("") === "index.html",
    "empty path → index.html (after manifest)",
  );

  // Loading a manifest must not change how unclaimed extensions are handled —
  // ".media" is in no codec's extension set, so it is still a directory.
  check(
    BeliefBaseWasm.normalize_path_extension("catalog/widget.media") ===
      "catalog/widget.media/index.html",
    "dotted directory → /index.html (after manifest)",
  );

  // ── 5. Edge cases and error handling ────────────────────────────────────
  console.log(`\n${BLUE}[5/5] Edge cases and error handling${RESET}`);

  // setKnownExtensions with custom extensions simulating an app shim
  const customManifest = JSON.stringify({
    version: "1",
    document_extensions: ["md", "xlsx", "ods", "yaml", "yml", "h", "c"],
  });
  BeliefBaseWasm.setKnownExtensions(customManifest);
  assert(true, "setKnownExtensions() with custom manifest succeeded");

  check(
    BeliefBaseWasm.normalize_path_extension("include/PeriodicLoop.h") ===
      "include/PeriodicLoop.html",
    ".h → .html (custom extension from app shim)",
  );
  check(
    BeliefBaseWasm.normalize_path_extension("src/main.c") === "src/main.html",
    ".c → .html (custom extension from app shim)",
  );

  // .yaml should still work after overwrite
  check(
    BeliefBaseWasm.normalize_path_extension("config/data.yaml") === "config/data.html",
    ".yaml → .html (still works after custom manifest)",
  );

  // Malformed JSON should throw (or return an error)
  let malformedThrew = false;
  try {
    BeliefBaseWasm.setKnownExtensions("not valid json");
  } catch (e) {
    malformedThrew = true;
    log(`Malformed JSON correctly rejected: ${e}`, "info");
  }
  check(malformedThrew, "setKnownExtensions rejects malformed JSON");

  // After the error, the previous extensions should still work
  // (the error shouldn't have cleared the registry)
  check(
    BeliefBaseWasm.normalize_path_extension("include/PeriodicLoop.h") ===
      "include/PeriodicLoop.html",
    ".h still works after failed setKnownExtensions",
  );

  // ── Summary ─────────────────────────────────────────────────────────────
  console.log(`\n${BLUE}=== Test Summary ===${RESET}`);
  console.log(`${GREEN}Passed: ${testsPassed}${RESET}`);
  if (testsFailed > 0) {
    console.log(`${RED}Failed: ${testsFailed}${RESET}`);
  }

  if (testsFailed === 0) {
    console.log(`\n${GREEN}✓ All tests passed!${RESET}`);
    console.log(`${GREEN}✓ codecs.json written correctly by export pipeline${RESET}`);
    console.log(`${GREEN}✓ setKnownExtensions loads manifest into WASM runtime${RESET}`);
    console.log(
      `${GREEN}✓ normalize_path_extension correctly rewrites custom extensions${RESET}`,
    );
    console.log(`${GREEN}✓ Error handling for malformed input is correct${RESET}`);
    process.exit(0);
  } else {
    console.log(`\n${RED}✗ Some tests failed${RESET}`);
    process.exit(1);
  }
}

runTests().catch((error) => {
  console.error(`\n${RED}✗ Test error: ${error.message}${RESET}`);
  console.error(error.stack);
  process.exit(1);
});
