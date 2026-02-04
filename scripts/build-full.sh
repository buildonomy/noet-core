#!/usr/bin/env bash
# Build script for noet-core with full features (CLI + daemon + WASM)
#
# This script builds WASM first (with clean feature set), then builds the
# main binary with both bin and service features, using the pre-built WASM.
#
# Usage:
#   ./scripts/build-full.sh           # Build in debug mode
#   ./scripts/build-full.sh --release # Build in release mode

set -e

# Colors for output
GREEN='\033[0;32m'
BLUE='\033[0;34m'
YELLOW='\033[1;33m'
RED='\033[0;31m'
NC='\033[0m' # No Color

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
PROJECT_ROOT="$(cd "$SCRIPT_DIR/.." && pwd)"

# Parse arguments
RELEASE_FLAG=""
CARGO_PROFILE="debug"
if [ "$1" = "--release" ]; then
    RELEASE_FLAG="--release"
    CARGO_PROFILE="release"
fi

echo -e "${BLUE}=== Building noet-core with full features ===${NC}\n"

cd "$PROJECT_ROOT"

# Step 1: Build WASM module
echo -e "${BLUE}[1/2] Building WASM module...${NC}"
echo -e "${YELLOW}Note: WASM builds with --features wasm --no-default-features${NC}"
echo -e "${YELLOW}      (no service/sqlx/tokio features for browser target)${NC}\n"

wasm-pack build --target web --out-dir pkg -- --features wasm --no-default-features $RELEASE_FLAG

if [ $? -eq 0 ]; then
    echo -e "\n${GREEN}✓ WASM build successful${NC}"
    echo -e "${GREEN}  Output: pkg/noet_core.js, pkg/noet_core_bg.wasm${NC}\n"
else
    echo -e "\n${RED}✗ WASM build failed${NC}"
    exit 1
fi

# Step 2: Build main binary with bin + service features
echo -e "${BLUE}[2/2] Building noet CLI binary with service features...${NC}"
echo -e "${YELLOW}Note: Using pre-built WASM from pkg/ (build.rs will skip rebuild)${NC}\n"

cargo build --features "bin service" $RELEASE_FLAG

if [ $? -eq 0 ]; then
    echo -e "\n${GREEN}✓ Binary build successful${NC}"
    echo -e "${GREEN}  Output: target/$CARGO_PROFILE/noet${NC}\n"
else
    echo -e "\n${RED}✗ Binary build failed${NC}"
    exit 1
fi

# Summary
echo -e "${GREEN}=== Build complete ===${NC}\n"
echo -e "Binary location: ${BLUE}target/$CARGO_PROFILE/noet${NC}"
echo -e "WASM artifacts:  ${BLUE}pkg/noet_core.js, pkg/noet_core_bg.wasm${NC}"
echo -e "\nFeatures enabled:"
echo -e "  ✓ CLI (parse, generate HTML)"
echo -e "  ✓ Service (daemon, file watching, SQLite)"
echo -e "  ✓ WASM (embedded for HTML viewer)"
echo -e "\nRun with:"
echo -e "  ${BLUE}./target/$CARGO_PROFILE/noet --help${NC}"
