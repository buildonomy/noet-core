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
//! ## Staleness detection
//!
//! We always run the inner `cargo build` (cargo's incremental compilation handles
//! no-op cases efficiently). To avoid re-running wasm-bindgen unnecessarily, we
//! write a fingerprint file (`target/wasm-build/pkg/wasm-bindgen.fingerprint`)
//! containing the mtime of the compiled wasm at the time wasm-bindgen last ran.
//! On subsequent builds we compare the current compiled wasm mtime against the
//! stored fingerprint; if they match, wasm-bindgen is skipped.
//!
//! This avoids the previous flawed approach of comparing pkg/ mtime against src/
//! mtime, which broke when a native `cargo build --lib` touched target/ files
//! without changing any sources.
//!
//! ## Troubleshooting Build Issues
//!
//! If you encounter problems:
//! 1. Clean all build artifacts: `cargo clean` (this includes target/wasm-build/)
//! 2. Verify wasm-bindgen is installed: `cargo install wasm-bindgen-cli`
//! 3. Check that wasm32-unknown-unknown target is installed: `rustup target add wasm32-unknown-unknown`

use std::env;
use std::io::Read;
use std::path::PathBuf;
use std::process::Command;

/// Compute a simple FNV-1a 64-bit hash of a file's contents.
/// Used to detect whether the compiled wasm has actually changed between builds,
/// since cargo updates artifact mtimes on every build regardless of whether
/// anything was recompiled.
fn hash_file(path: &PathBuf) -> Option<u64> {
    let mut file = std::fs::File::open(path).ok()?;
    let mut hasher: u64 = 0xcbf29ce484222325; // FNV offset basis
    let mut buf = [0u8; 65536];
    loop {
        let n = file.read(&mut buf).ok()?;
        if n == 0 {
            break;
        }
        for &byte in &buf[..n] {
            hasher ^= byte as u64;
            hasher = hasher.wrapping_mul(0x100000001b3); // FNV prime
        }
    }
    Some(hasher)
}

/// Read a stored hash fingerprint from a file.
fn read_fingerprint(path: &PathBuf) -> Option<u64> {
    std::fs::read_to_string(path)
        .ok()
        .and_then(|s| s.trim().parse::<u64>().ok())
}

/// Write a hash fingerprint to a file.
fn write_fingerprint(path: &PathBuf, hash: u64) {
    let _ = std::fs::write(path, hash.to_string());
}

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

    // Always run the inner cargo build — cargo's own incremental compilation
    // handles the "nothing changed" case efficiently and correctly. Our previous
    // src/ mtime heuristic was unreliable: a native `cargo build --lib` can
    // touch files under target/ without changing src/, making pkg/ appear newer
    // than sources and causing the WASM build to be skipped even after real changes.
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

    // Step 2: Generate JavaScript bindings with wasm-bindgen.
    //
    // Staleness check: we store the mtime of the compiled wasm at the time
    // wasm-bindgen last ran in a fingerprint file. If the current compiled wasm
    // mtime matches the stored fingerprint, wasm-bindgen output is still valid.
    //
    // We cannot rely on comparing pkg/ mtime vs deps/ mtime directly because
    // cargo updates the deps/ artifact mtime on every successful build (even
    // no-ops), which would always make it appear newer than pkg/.

    // Create pkg directory if it doesn't exist
    std::fs::create_dir_all(&pkg_dir).expect("Failed to create pkg/ directory");

    // Prefer the deps/ artifact (always updated by cargo) over the top-level
    // hardlink (whose mtime cargo does not update on no-op builds).
    let deps_wasm = wasm_target_dir
        .join("wasm32-unknown-unknown")
        .join("debug")
        .join("deps")
        .join("noet_core.wasm");
    let wasm_input = if deps_wasm.exists() {
        deps_wasm.clone()
    } else {
        wasm_target_dir
            .join("wasm32-unknown-unknown")
            .join("debug")
            .join("noet_core.wasm")
    };

    let fingerprint_file = pkg_dir.join("wasm-bindgen.fingerprint");
    let skip_bindgen = if wasm_file.exists() && js_file.exists() && wasm_input.exists() {
        // Compare stored hash against current compiled wasm hash.
        // Cargo updates artifact mtimes on every build even when nothing recompiled,
        // so mtime comparison is unreliable — content hash is the correct signal.
        let stored = read_fingerprint(&fingerprint_file);
        let current = hash_file(&wasm_input);
        matches!((stored, current), (Some(s), Some(c)) if s == c)
    } else {
        false
    };

    if skip_bindgen {
        println!("cargo:warning=WASM build complete (wasm unchanged, skipping wasm-bindgen)");
        return;
    }

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
            // Record the compiled wasm hash so we can skip wasm-bindgen next
            // time if the wasm content hasn't changed.
            if let Some(hash) = hash_file(&wasm_input) {
                write_fingerprint(&fingerprint_file, hash);
            }
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
