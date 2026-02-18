//! Build script for noet-core
//!
//! This script checks for pre-built WASM artifacts or builds them if missing.
//! WASM artifacts (noet_core.js, noet_core_bg.wasm) in pkg/ are embedded into
//! the binary via src/codec/assets.rs.
//!
//! ## WASM Build Strategy
//!
//! For distribution (crates.io), WASM artifacts should be pre-built and included:
//! 1. Run: `./scripts/build-full.sh` (or manually build WASM as shown below)
//! 2. Commit pkg/ directory
//! 3. Publish to crates.io with pre-built artifacts
//!
//! For development, this script will build WASM if pkg/ is missing using:
//!   cargo build --target wasm32-unknown-unknown --no-default-features --features wasm
//!   wasm-bindgen target/wasm32-unknown-unknown/debug/noet_core.wasm --out-dir pkg --target web
//!
//! ## Troubleshooting Build Hangs
//!
//! If builds hang with "waiting for file lock on artifact directory":
//! 1. Kill stale cargo processes: `killall cargo`
//! 2. Clean build state: `cargo clean`
//! 3. Remove WASM artifacts: `rm -rf pkg/`
//! 4. Retry build
//!
//! This happens when wasm-pack gets interrupted (Ctrl+C, timeout, crash) and
//! leaves stale locks in target/.cargo-lock.

use std::env;
use std::path::PathBuf;
use std::process::Command;

fn main() {
    println!("cargo:rerun-if-changed=src/wasm.rs");
    println!("cargo:rerun-if-changed=src/properties.rs");
    println!("cargo:rerun-if-changed=src/beliefbase.rs");
    println!("cargo:rerun-if-changed=Cargo.toml");

    // Guard against incompatible feature combinations
    // The `wasm` and `service` features are mutually exclusive
    let has_wasm = env::var("CARGO_FEATURE_WASM").is_ok();
    let has_service = env::var("CARGO_FEATURE_SERVICE").is_ok();

    if has_wasm && has_service {
        eprintln!("\n=== ERROR ===");
        eprintln!("Incompatible feature combination detected!");
        eprintln!("Cannot build with both 'wasm' and 'service' features enabled.");
        eprintln!("\nThe 'wasm' feature (for browser WASM) is incompatible with:");
        eprintln!("  - 'service' (filesystem, tokio runtime, sqlx, file watching)");
        eprintln!("\nValid build commands:");
        eprintln!("  cargo build --features bin              # CLI with WASM for HTML generation");
        eprintln!("  cargo build --features service          # Library with service features");
        eprintln!("  cargo build --no-default-features       # Library only");
        eprintln!("\nFor full features (CLI + daemon + WASM):");
        eprintln!("  ./scripts/build-full.sh                 # Two-phase build");
        eprintln!("\nDo NOT use:");
        eprintln!("  cargo build --all-features              # ❌ Invalid combination");
        eprintln!("  cargo build --features \"wasm service\"   # ❌ Invalid combination");
        eprintln!("=============\n");
        panic!("Incompatible features: wasm + service");
    }

    // Check if we should verify/build WASM
    // WASM is needed when the bin feature is enabled (for CLI with HTML generation)
    let should_have_wasm = env::var("CARGO_FEATURE_BIN").is_ok();

    if !should_have_wasm {
        println!("cargo:warning=Skipping WASM check (bin feature not enabled)");
        println!("cargo:warning=WASM is only needed when building the CLI binary");
        return;
    }

    // Check if wasm-bindgen is installed
    let wasm_bindgen_check = Command::new("wasm-bindgen").arg("--version").output();

    match wasm_bindgen_check {
        Ok(output) if output.status.success() => {
            let version = String::from_utf8_lossy(&output.stdout);
            println!("cargo:warning=Found wasm-bindgen: {}", version.trim());
        }
        _ => {
            eprintln!("\n=== ERROR ===");
            eprintln!("wasm-bindgen is not installed or not in PATH");
            eprintln!("\nTo install wasm-bindgen-cli:");
            eprintln!("  cargo install wasm-bindgen-cli");
            eprintln!("\nAlternatively, build without WASM support:");
            eprintln!("  cargo build --no-default-features");
            eprintln!("\nOr use the build script that handles everything:");
            eprintln!("  ./scripts/build-full.sh");
            eprintln!("=============\n");
            panic!("wasm-bindgen is required to build noet-core with WASM support");
        }
    }

    // Get the manifest directory (project root)
    let manifest_dir = PathBuf::from(env::var("CARGO_MANIFEST_DIR").unwrap());
    let pkg_dir = manifest_dir.join("pkg");

    // Check if pkg/ already exists and has fresh artifacts
    let wasm_file = pkg_dir.join("noet_core_bg.wasm");
    let js_file = pkg_dir.join("noet_core.js");

    let artifacts_exist = wasm_file.exists() && js_file.exists();

    // Check if artifacts exist (pre-built or from previous build)
    if artifacts_exist {
        println!("cargo:warning=✓ Using pre-built WASM artifacts from pkg/");
        println!("cargo:warning=  (noet_core.js, noet_core_bg.wasm)");
        return;
    }

    // Artifacts don't exist - need to build them
    println!("cargo:warning=WASM artifacts not found in pkg/");
    println!("cargo:warning=Building WASM module with cargo + wasm-bindgen...");
    println!("cargo:warning=");

    // Step 1: Build WASM module with cargo
    println!("cargo:warning=Step 1: Building WASM with cargo...");
    let cargo_status = Command::new("cargo")
        .current_dir(&manifest_dir)
        .arg("build")
        .arg("--target")
        .arg("wasm32-unknown-unknown")
        .arg("--no-default-features")
        .arg("--features")
        .arg("wasm")
        .status();

    match cargo_status {
        Ok(status) if status.success() => {
            println!("cargo:warning=✓ WASM cargo build successful");
        }
        Ok(status) => {
            eprintln!("\n=== ERROR ===");
            eprintln!(
                "WASM cargo build failed with exit code: {:?}",
                status.code()
            );
            eprintln!("\nTry running manually:");
            eprintln!("  cargo build --target wasm32-unknown-unknown --no-default-features --features wasm");
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
    println!("cargo:warning=Step 2: Generating JavaScript bindings...");

    // Create pkg directory if it doesn't exist
    std::fs::create_dir_all(&pkg_dir).expect("Failed to create pkg/ directory");

    let wasm_input = manifest_dir
        .join("target")
        .join("wasm32-unknown-unknown")
        .join("debug")
        .join("noet_core.wasm");

    let bindgen_status = Command::new("wasm-bindgen")
        .current_dir(&manifest_dir)
        .arg(&wasm_input)
        .arg("--out-dir")
        .arg("pkg")
        .arg("--target")
        .arg("web")
        .status();

    match bindgen_status {
        Ok(status) if status.success() => {
            println!("cargo:warning=✓ wasm-bindgen successful");
        }
        Ok(status) => {
            eprintln!("\n=== ERROR ===");
            eprintln!("wasm-bindgen failed with exit code: {:?}", status.code());
            eprintln!("\nTry running manually:");
            eprintln!("  wasm-bindgen target/wasm32-unknown-unknown/debug/noet_core.wasm --out-dir pkg --target web");
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
        panic!("WASM build succeeded but pkg/noet_core_bg.wasm not found");
    }
    if !js_file.exists() {
        panic!("WASM build succeeded but pkg/noet_core.js not found");
    }

    println!("cargo:warning=✓ WASM artifacts ready for embedding");
}
