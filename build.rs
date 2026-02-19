//! Build script for noet-core
//!
//! This script checks for pre-built WASM artifacts or builds them if missing.
//! WASM artifacts (noet_core.js, noet_core_bg.wasm) in pkg/ are embedded into
//! the binary via src/codec/assets.rs.
//!
//! ## WASM Build Strategy
//!
//! **Standard approach:**
//!   cargo build --features bin                # Automatically builds WASM if missing
//!   cargo build --features "bin service"      # Full features including daemon
//!
//! **For distribution (crates.io):**
//! 1. Pre-build WASM: `cargo build --features bin`
//! 2. Commit target/wasm-build/pkg/ directory
//! 3. Publish to crates.io with pre-built artifacts
//!
//! ## How Automatic WASM Build Works
//!
//! When `bin` feature is enabled and target/wasm-build/pkg/ is missing, this script:
//! 1. Uses isolated target directory (target/wasm-build/) to avoid file locks
//! 2. Clears CARGO_FEATURE_SERVICE/BIN env vars (prevents feature conflicts)
//! 3. Spawns: `cargo build --target wasm32-unknown-unknown --no-default-features --features wasm`
//! 4. Runs: `wasm-bindgen target/wasm-build/wasm32-unknown-unknown/debug/noet_core.wasm --out-dir target/wasm-build/pkg --target web`
//! 5. Original build continues with WASM artifacts embedded from target/wasm-build/pkg/
//!
//! The isolated target directory eliminates file lock conflicts between parent and nested builds.
//!
//! ## Troubleshooting Build Issues
//!
//! If you encounter problems:
//! 1. Clean all build artifacts: `cargo clean` (this includes target/wasm-build/)
//! 2. Verify wasm-bindgen is installed: `cargo install wasm-bindgen-cli`
//! 3. Check that wasm32-unknown-unknown target is installed: `rustup target add wasm32-unknown-unknown`

use std::env;
use std::path::PathBuf;
use std::process::Command;

fn main() {
    println!("cargo:rerun-if-changed=src/wasm.rs");
    println!("cargo:rerun-if-changed=src/properties.rs");
    println!("cargo:rerun-if-changed=src/beliefbase.rs");
    println!("cargo:rerun-if-changed=Cargo.toml");

    // Check if we should verify/build WASM
    // WASM is needed when the bin feature is enabled (for CLI with HTML generation)
    let should_have_wasm = env::var("CARGO_FEATURE_BIN").is_ok();

    if !should_have_wasm {
        // Not building with bin feature - skip WASM build
        return;
    }

    // Check if wasm-bindgen is installed
    let wasm_bindgen_check = Command::new("wasm-bindgen").arg("--version").output();

    match wasm_bindgen_check {
        Ok(output) if output.status.success() => {
            // wasm-bindgen available
        }
        _ => {
            eprintln!("\n=== ERROR ===");
            eprintln!("wasm-bindgen is not installed or not in PATH");
            eprintln!("\nTo install wasm-bindgen-cli:");
            eprintln!("  cargo install wasm-bindgen-cli");
            eprintln!("\nAlternatively, build without WASM support:");
            eprintln!("  cargo build --features service");
            eprintln!("=============\n");
            panic!("wasm-bindgen is required to build noet-core with WASM support");
        }
    }

    // Get the manifest directory (project root)
    let manifest_dir = PathBuf::from(env::var("CARGO_MANIFEST_DIR").unwrap());

    // Use wasm-build target directory for both compilation and pkg/ artifacts
    let wasm_target_dir = manifest_dir.join("target").join("wasm-build");
    let pkg_dir = wasm_target_dir.join("pkg");

    // Check if pkg/ already exists and has fresh artifacts
    let wasm_file = pkg_dir.join("noet_core_bg.wasm");
    let js_file = pkg_dir.join("noet_core.js");

    let artifacts_exist = wasm_file.exists() && js_file.exists();

    // Check if artifacts exist (pre-built or from previous build)
    if artifacts_exist {
        // Using cached WASM - no output needed
        return;
    }

    // Artifacts don't exist - need to build them
    println!("cargo:warning=Compiling WASM module for HTML generation...");

    // Clear parent build's feature flags to avoid inheritance
    // The nested WASM build uses isolated features (wasm only), so we clear
    // the parent's BIN and SERVICE flags to prevent feature conflicts
    let cargo_output = Command::new("cargo")
        .current_dir(&manifest_dir)
        .env("CARGO_TARGET_DIR", &wasm_target_dir)
        .env_remove("CARGO_FEATURE_BIN")
        .env_remove("CARGO_FEATURE_SERVICE")
        .env_remove("CARGO_ENCODED_RUSTFLAGS")
        .env_remove("RUSTFLAGS")
        .arg("build")
        .arg("--target")
        .arg("wasm32-unknown-unknown")
        .arg("--no-default-features")
        .arg("--features")
        .arg("wasm")
        .output();

    match cargo_output {
        Ok(output) if output.status.success() => {
            // WASM compilation successful
        }
        Ok(output) => {
            eprintln!("\n=== ERROR ===");
            eprintln!(
                "WASM cargo build failed with exit code: {:?}",
                output.status.code()
            );
            eprintln!("\n--- STDOUT ---");
            eprintln!("{}", String::from_utf8_lossy(&output.stdout));
            eprintln!("\n--- STDERR ---");
            eprintln!("{}", String::from_utf8_lossy(&output.stderr));
            eprintln!("\nTry running manually:");
            eprintln!("  CARGO_TARGET_DIR=target/wasm-build cargo build --target wasm32-unknown-unknown --no-default-features --features wasm");
            eprintln!("=============\n");
            panic!("WASM cargo build failed");
        }
        Err(e) => {
            eprintln!("\n=== ERROR ===");
            eprintln!("Failed to execute cargo: {}", e);
            eprintln!("=============\n");
            panic!("Failed to execute cargo");
        }
    }

    // Step 2: Generate JavaScript bindings with wasm-bindgen

    // Create pkg directory if it doesn't exist
    std::fs::create_dir_all(&pkg_dir).expect("Failed to create pkg/ directory");

    let wasm_input = wasm_target_dir
        .join("wasm32-unknown-unknown")
        .join("debug")
        .join("noet_core.wasm");

    let bindgen_output = Command::new("wasm-bindgen")
        .current_dir(&manifest_dir)
        .arg(&wasm_input)
        .arg("--out-dir")
        .arg(&pkg_dir)
        .arg("--target")
        .arg("web")
        .output();

    match bindgen_output {
        Ok(output) if output.status.success() => {
            println!("cargo:warning=WASM build complete");
        }
        Ok(output) => {
            eprintln!("\n=== ERROR ===");
            eprintln!(
                "wasm-bindgen failed with exit code: {:?}",
                output.status.code()
            );
            eprintln!("\n--- STDOUT ---");
            eprintln!("{}", String::from_utf8_lossy(&output.stdout));
            eprintln!("\n--- STDERR ---");
            eprintln!("{}", String::from_utf8_lossy(&output.stderr));
            eprintln!("\nTry running manually:");
            eprintln!("  wasm-bindgen target/wasm-build/wasm32-unknown-unknown/debug/noet_core.wasm --out-dir target/wasm-build/pkg --target web");
            eprintln!("=============\n");
            panic!("wasm-bindgen failed");
        }
        Err(e) => {
            eprintln!("\n=== ERROR ===");
            eprintln!("Failed to execute wasm-bindgen: {}", e);
            eprintln!("=============\n");
            panic!("Failed to execute wasm-bindgen");
        }
    }

    // Verify artifacts were created
    if !wasm_file.exists() {
        panic!("WASM build succeeded but target/wasm-build/pkg/noet_core_bg.wasm not found");
    }
    if !js_file.exists() {
        panic!("WASM build succeeded but target/wasm-build/pkg/noet_core.js not found");
    }
}
