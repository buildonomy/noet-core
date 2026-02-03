# Noet WASM Browser Test Suite

This directory contains browser-based integration tests for the Noet WASM module.

## Overview

The test suite validates that:
1. WASM module compiles and loads correctly
2. `beliefbase.json` exports are valid and parseable
3. All WASM APIs work in a browser environment
4. Console logging provides useful debugging output

## Quick Start

```bash
# From project root
./tests/browser/run.sh
```

This script will:
1. Build the WASM module (`wasm-pack build`)
2. Generate test data from `tests/network_1` fixtures
3. Export `beliefbase.json` to `tests/browser/test-output/`
4. Start HTTP server on `localhost:8000`
5. Open your browser to the test runner

## Test Runner

The test runner (`test_runner.html`) performs the following tests:

### Test 1: Basic Queries
- Verify `node_count()` returns positive value
- Confirm WASM module loaded successfully

### Test 2: Document Queries
- Test `get_documents()` returns array
- Verify documents have expected structure (title, bid, etc.)

### Test 3: Network Queries
- Test `get_networks()` returns array
- Verify network nodes exist

### Test 4: Node Retrieval
- Test `get_by_bid()` with valid BID
- Verify retrieved node matches original

### Test 5: Search Functionality
- Test `search()` with substring query
- Verify results array is valid

### Test 6: Backlinks
- Test `get_backlinks()` returns array
- Verify backlink relationships exist

### Test 7: Forward Links
- Test `get_forward_links()` returns array
- Verify forward link relationships exist

### Test 8: Error Handling
- Test invalid BID handling (should return null/empty array)
- Verify graceful error behavior

## Console Logging

All WASM methods log to browser console with emoji prefixes:

- `✅` Success operations
- `❌` Errors
- `⚠️` Warnings (invalid input, not found)
- `🔍` Query/search operations
- `📊` Statistics (node counts, result counts)

**Open browser DevTools** to see detailed logs during test execution.

## Manual Testing

To manually test WASM APIs:

1. Run `./tests/browser/run.sh`
2. Open browser DevTools Console
3. Run commands:

```javascript
// Get the BeliefBaseWasm instance (created by test runner)
// You'll need to modify test_runner.html to expose it globally:
// window.testBb = bb;

// Then in console:
const docs = testBb.get_documents();
console.log(docs);

const results = testBb.search("test");
console.log(results);

const backlinks = testBb.get_backlinks(docs[0].bid);
console.log(backlinks);
```

## File Structure

```
tests/browser/
├── README.md              # This file
├── run.sh                 # Test runner script
├── test_runner.html       # Browser test page
└── test-output/           # Generated test data (gitignored)
    ├── beliefbase.json    # Exported belief graph
    ├── *.html             # Generated HTML documents
    └── assets/            # Hardlinked assets
```

## Test Data Source

Tests use the `tests/network_1/` fixture, which contains:
- Multiple markdown documents
- Cross-document links
- Assets (images, etc.)
- Network relationships

This provides a realistic test environment for WASM functionality.

## Troubleshooting

### WASM module won't load
- Check browser console for CORS errors
- Ensure HTTP server is running (not `file://` protocol)
- Verify `pkg/noet_core_bg.wasm` exists and is ~2.1MB

### beliefbase.json not found
- Run `./target/debug/noet parse tests/network_1 --html-output tests/browser/test-output`
- Verify JSON file is exported (check logs for "Exported BeliefGraph")

### Tests fail with "Module not found"
- Ensure WASM was built with: `wasm-pack build --target web`
- Check that `pkg/noet_core.js` exists

### Console shows no logs
- WASM methods log to console automatically
- Open DevTools *before* running tests
- Check Console tab (not Network or Elements)

## CI Integration

To run tests in CI (headless):

```bash
# Install Chrome/Firefox headless
# Run server in background
python3 -m http.server 8000 &
SERVER_PID=$!

# Run headless browser tests (future: use Playwright/Puppeteer)
# For now, manual browser testing only

# Cleanup
kill $SERVER_PID
```

**TODO**: Add headless browser automation (Playwright, Selenium, or Puppeteer).

## Next Steps (Step 8)

Once WASM validation is complete, Step 8 will add:
- Interactive metadata panel UI
- Two-click navigation (activate → navigate)
- SPA-style history management
- Client-side search interface
- Backlink/forward link panels

This test infrastructure will expand to validate those features.