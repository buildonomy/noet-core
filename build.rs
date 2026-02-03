//! Build script for noet-core
//!
//! This script automates WASM compilation using wasm-pack before the main build.
//! WASM artifacts (noet_core.js, noet_core_bg.wasm) are generated in pkg/ and
//! then embedded into the binary via src/codec/assets.rs.

use std::env;
use std::path::PathBuf;
use std::process::Command;

fn main() {
    println!("cargo:rerun-if-changed=src/wasm.rs");
    println!("cargo:rerun-if-changed=src/properties.rs");
    println!("cargo:rerun-if-changed=src/beliefbase.rs");
    println!("cargo:rerun-if-changed=Cargo.toml");

    // Check if we should build WASM
    // We build WASM when the bin feature is enabled (for CLI with HTML generation)
    // Note: WASM is compiled with --features wasm --no-default-features (different from main build)
    let should_build_wasm = env::var("CARGO_FEATURE_BIN").is_ok();

    if !should_build_wasm {
        println!("cargo:warning=Skipping WASM build (bin feature not enabled)");
        println!("cargo:warning=WASM is only needed when building the CLI binary");
        return;
    }

    println!("cargo:warning=Building WASM module with wasm-pack...");

    // Check if wasm-pack is installed
    let wasm_pack_check = Command::new("wasm-pack").arg("--version").output();

    match wasm_pack_check {
        Ok(output) if output.status.success() => {
            let version = String::from_utf8_lossy(&output.stdout);
            println!("cargo:warning=Found wasm-pack: {}", version.trim());
        }
        _ => {
            eprintln!("\n=== ERROR ===");
            eprintln!("wasm-pack is not installed or not in PATH");
            eprintln!("\nTo install wasm-pack:");
            eprintln!("  curl https://rustwasm.github.io/wasm-pack/installer/init.sh -sSf | sh");
            eprintln!("\nOr visit: https://rustwasm.github.io/wasm-pack/installer/");
            eprintln!("\nAlternatively, build without WASM support:");
            eprintln!("  cargo build --no-default-features");
            eprintln!("=============\n");
            panic!("wasm-pack is required to build noet-core with WASM support");
        }
    }

    // Get the manifest directory (project root)
    let manifest_dir = PathBuf::from(env::var("CARGO_MANIFEST_DIR").unwrap());
    let pkg_dir = manifest_dir.join("pkg");

    // Check if pkg/ already exists and has fresh artifacts
    let wasm_file = pkg_dir.join("noet_core_bg.wasm");
    let js_file = pkg_dir.join("noet_core.js");

    let artifacts_exist = wasm_file.exists() && js_file.exists();

    // Only rebuild if artifacts don't exist
    // (cargo will handle rerun-if-changed triggers for incremental builds)
    if artifacts_exist {
        println!("cargo:warning=WASM artifacts already exist in pkg/, skipping rebuild");
        println!("cargo:warning=Delete pkg/ to force rebuild");
        return;
    }

    println!("cargo:warning=Running wasm-pack build...");

    // Run wasm-pack build
    let status = Command::new("wasm-pack")
        .current_dir(&manifest_dir)
        .arg("build")
        .arg("--target")
        .arg("web")
        .arg("--")
        .arg("--features")
        .arg("wasm")
        .arg("--no-default-features")
        .status();

    match status {
        Ok(status) if status.success() => {
            println!("cargo:warning=✓ WASM build successful");
            println!("cargo:warning=  Output: pkg/noet_core.js, pkg/noet_core_bg.wasm");
        }
        Ok(status) => {
            eprintln!("\n=== ERROR ===");
            eprintln!("wasm-pack build failed with exit code: {:?}", status.code());
            eprintln!("=============\n");
            panic!("WASM build failed");
        }
        Err(e) => {
            eprintln!("\n=== ERROR ===");
            eprintln!("Failed to execute wasm-pack: {}", e);
            eprintln!("=============\n");
            panic!("Failed to execute wasm-pack");
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
