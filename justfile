# noet-core — developer task runner
#
# Requires: just (https://github.com/casey/just)
#   cargo install just
#
# ## This file is the single source of truth for CI commands.
#
# `.github/workflows/test-selfhosted.yml` does not spell out build/test/lint
# commands — each job step invokes a recipe here. Change a command in this file
# and CI picks it up; there is no second copy to keep in sync. The inverse
# (a justfile that reads the workflow YAML) does not work: the workflow's
# toolchain-install steps are not locally runnable, and replaying it would mean
# reimplementing matrix expansion, `${{ }}` substitution and per-step `env:`.
#
# Recipes below are grouped to match CI job names, so `just lint` runs exactly
# what the `lint` job runs. `just ci` runs every gating job in sequence.
#
# Usage:
#   just                  List available recipes
#   just ci               Run every gating CI job locally (slow)
#   just lint             rustfmt + clippy, as CI runs them
#   just docs             rustdoc with -D warnings, as CI runs them
#   just test-matrix      All three feature combinations, as CI runs them
#   just test             Tests with default features only (fast inner loop)

# Default recipe: show what's available.
default:
    @just --list

# ---------------------------------------------------------------------------
# Setup
# ---------------------------------------------------------------------------

# Install the toolchain pieces CI expects (wasm target, wasm-bindgen, components).
deps:
    rustup target add wasm32-unknown-unknown
    rustup component add rustfmt clippy
    @if ! command -v wasm-bindgen > /dev/null 2>&1; then cargo install wasm-bindgen-cli --version 0.2.108; else echo "wasm-bindgen already present"; fi

# Fetch vendored third-party binaries (miniserve, for the `distribute` feature).
vendor:
    ./vendor/fetch.sh

# ---------------------------------------------------------------------------
# CI job: test-matrix
#
# Feature combinations, and why each one earns its place:
#   --no-default-features                      library-only smoke test
#   --no-default-features --features bin,service   full CLI minus git-tracking
#   (empty)                                    default developer build
# ---------------------------------------------------------------------------

# Build one feature combination.
build flags='':
    cargo build --verbose {{ flags }}

# Test one feature combination.
test flags='':
    cargo test --verbose {{ flags }}

# Run doc tests for one feature combination.
doc-test flags='':
    cargo test --doc {{ flags }}

# Full matrix: build + test + doc-test across all three feature combinations.
test-matrix:
    just build '--no-default-features'
    just test '--no-default-features'
    just doc-test '--no-default-features'
    just build '--no-default-features --features bin,service'
    just test '--no-default-features --features bin,service'
    just doc-test '--no-default-features --features bin,service'
    just build ''
    just test ''
    just doc-test ''

# ---------------------------------------------------------------------------
# CI job: lint
# ---------------------------------------------------------------------------

# Check formatting without modifying files.
fmt-check:
    cargo fmt --all -- --check

# Apply formatting. Not a CI step — CI only checks.
fmt:
    cargo fmt --all

# Clippy over every target and feature, warnings denied.
clippy:
    cargo clippy --all-features --all-targets -- -D warnings

# rustfmt + clippy, matching the `lint` CI job.
lint: fmt-check clippy

# Not a CI step. `clippy --all-features` on a native host does NOT check the
# `wasm` module, which is gated on `target_arch = "wasm32"`. CI catches wasm
# breakage only indirectly, via the nested WASM build in build.rs.

# Clippy for the wasm32 target — checks code the native lint job cannot see.
clippy-wasm:
    cargo clippy --target wasm32-unknown-unknown --features wasm -- -D warnings

# ---------------------------------------------------------------------------
# CI job: docs
# ---------------------------------------------------------------------------

# Build rustdoc with warnings denied.
docs:
    RUSTDOCFLAGS="-D warnings" cargo doc --no-deps --all-features

# Build rustdoc and open it. Not a CI step.
docs-open:
    RUSTDOCFLAGS="-D warnings" cargo doc --no-deps --all-features --open

# ---------------------------------------------------------------------------
# CI job: examples
# ---------------------------------------------------------------------------

# Build every example, then run the smoke-test example.
examples:
    cargo build --examples
    cargo run --example basic_usage

# ---------------------------------------------------------------------------
# CI job: wasm-interface
#
# Drives the built WASM bundle through node against a parsed fixture network,
# checking the browser-facing interface that Rust-side tests cannot reach.
# ---------------------------------------------------------------------------

# Build the CLI (building the WASM module) and run the browser interface tests.
wasm-interface:
    #!/usr/bin/env bash
    set -euo pipefail
    if ! command -v node > /dev/null 2>&1; then
        echo "ERROR: node is required for the wasm-interface tests" >&2
        exit 1
    fi
    cargo build --features bin
    mkdir -p tests/browser/test-output
    ./target/debug/noet parse tests/network_1 --html-output tests/browser/test-output
    node tests/browser/test_related_nodes.js
    node tests/browser/test_nav_tree.js
    node tests/browser/test_codec_manifest.js

# ---------------------------------------------------------------------------
# CI job: standalone
#
# Proves noet-core builds and links as an ordinary path dependency, catching
# breakage that in-repo builds hide (missing re-exports, feature leakage from
# dev-dependencies).
# ---------------------------------------------------------------------------

# Scratch location for the generated consumer crate. Under target/, so it is
# already gitignored and `cargo clean` removes it.
standalone_dir := justfile_directory() / "target" / "standalone-test"

# Create a throwaway crate that depends on noet-core by path, then build and run it.
standalone:
    #!/usr/bin/env bash
    set -euo pipefail
    cargo build
    rm -rf "{{ standalone_dir }}"
    mkdir -p "{{ standalone_dir }}"
    cd "{{ standalone_dir }}"
    # --vcs none: a throwaway crate needs no repo, and creating one nests a git
    # dir inside target/ that later `rm -rf` cannot always remove.
    cargo init --vcs none --name standalone_test
    # Declare its own workspace root so cargo does not try to interpret the
    # enclosing noet-core package as a parent workspace.
    printf '\n[workspace]\n' >> Cargo.toml
    cargo add noet-core --path "{{ justfile_directory() }}"
    cat > src/main.rs << 'EOF'
    use noet_core::beliefbase::BeliefBase;

    fn main() {
        let bb = BeliefBase::default();
        println!("BeliefBase created: {} nodes and {} edges", bb.states().len(), bb.relations().as_graph().edge_count());
    }
    EOF
    cargo build --verbose
    cargo run

# ---------------------------------------------------------------------------
# Aggregate
# ---------------------------------------------------------------------------

# Run every gating CI job, in the order most likely to fail fast.
ci: lint docs examples test-matrix wasm-interface standalone

# The subset worth running before every push: fast, and catches most CI failures.
check: fmt-check clippy docs
