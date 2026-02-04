#!/usr/bin/env bash
# Browser test runner for Noet WASM module
#
# This script:
# 1. Builds the WASM module
# 2. Generates test HTML output from network_1 fixtures
# 3. Exports beliefbase.json
# 4. Serves the test runner on localhost:8000
# 5. Opens browser to test page

set -e

# Colors for output
GREEN='\033[0;32m'
BLUE='\033[0;34m'
YELLOW='\033[1;33m'
RED='\033[0;31m'
NC='\033[0m' # No Color

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
PROJECT_ROOT="$(cd "$SCRIPT_DIR/../.." && pwd)"
TEST_OUTPUT="$SCRIPT_DIR/test-output"

echo -e "${BLUE}=== Noet WASM Browser Test Runner ===${NC}\n"

# Step 1: Build noet CLI binary (build.rs will handle WASM compilation)
echo -e "${BLUE}[1/4] Building noet CLI binary (includes WASM via build.rs)...${NC}"
cd "$PROJECT_ROOT"

# Clean any stale locks
if pgrep -f "cargo.*wasm" > /dev/null; then
    echo -e "${YELLOW}⚠ Killing stale cargo processes...${NC}"
    pkill -f "cargo.*wasm"
    sleep 1
fi

cargo build --features bin

if [ $? -eq 0 ]; then
    echo -e "${GREEN}✓ Binary build successful${NC}\n"
else
    echo -e "${RED}✗ Binary build failed${NC}"
    exit 1
fi

# Step 2: Generate test output
echo -e "${BLUE}[2/4] Generating test HTML and JSON from network_1...${NC}"

# Clean old test output
rm -rf "$TEST_OUTPUT"
mkdir -p "$TEST_OUTPUT"

# Parse network_1 with HTML output
./target/debug/noet parse tests/network_1 --html-output "$TEST_OUTPUT" 2>&1 | grep -E "(Parsed|Exported|documents)"

if [ $? -eq 0 ]; then
    echo -e "${GREEN}✓ Test data generated${NC}\n"
else
    echo -e "${RED}✗ Failed to generate test data${NC}"
    exit 1
fi

# Step 3: Verify files exist
echo -e "${BLUE}[3/4] Verifying test assets...${NC}"

REQUIRED_FILES=(
    "$TEST_OUTPUT/beliefbase.json"
    "$PROJECT_ROOT/pkg/noet_core.js"
    "$PROJECT_ROOT/pkg/noet_core_bg.wasm"
    "$SCRIPT_DIR/test_runner.html"
)

ALL_PRESENT=true
for file in "${REQUIRED_FILES[@]}"; do
    if [ -f "$file" ]; then
        SIZE=$(du -h "$file" | cut -f1)
        echo -e "${GREEN}✓${NC} $(basename "$file") (${SIZE})"
    else
        echo -e "${RED}✗${NC} Missing: $file"
        ALL_PRESENT=false
    fi
done

if [ "$ALL_PRESENT" = false ]; then
    echo -e "\n${RED}✗ Required files missing${NC}"
    exit 1
fi

echo -e "\n${GREEN}✓ All test assets present${NC}\n"

# Step 4: Start HTTP server
echo -e "${BLUE}[4/4] Starting HTTP server...${NC}"
echo -e "${YELLOW}Server will run at: http://localhost:8000/tests/browser/test_runner.html${NC}"
echo -e "${YELLOW}Press Ctrl+C to stop${NC}\n"

# Check if python3 is available
if command -v python3 &> /dev/null; then
    cd "$PROJECT_ROOT"

    # Try to open browser automatically (works on macOS, Linux with xdg-open, WSL with explorer.exe)
    sleep 1
    if command -v open &> /dev/null; then
        open "http://localhost:8000/tests/browser/test_runner.html" 2>/dev/null &
    elif command -v xdg-open &> /dev/null; then
        xdg-open "http://localhost:8000/tests/browser/test_runner.html" 2>/dev/null &
    elif command -v explorer.exe &> /dev/null; then
        explorer.exe "http://localhost:8000/tests/browser/test_runner.html" 2>/dev/null &
    fi

    python3 -m http.server 8000
else
    echo -e "${RED}✗ python3 not found. Please install Python 3 or use another HTTP server.${NC}"
    exit 1
fi
