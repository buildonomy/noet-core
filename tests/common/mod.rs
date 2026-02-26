//! Shared test utilities for integration tests.
//!
//! Import from integration test files as:
//! ```ignore
//! mod common;
//! ```

use std::path::PathBuf;
use tempfile::TempDir;

/// Initialize tracing for tests, respecting RUST_LOG env var.
///
/// Safe to call multiple times — subsequent calls are no-ops.
#[allow(dead_code)]
pub fn init_logging() {
    tracing_subscriber::fmt()
        .with_env_filter(tracing_subscriber::EnvFilter::from_default_env())
        .with_test_writer()
        .try_init()
        .ok();
}

/// Create a test network directory with index.md and doc1.md.
///
/// Returns the path to the network directory (e.g. `<temp_dir>/test_network/`).
///
/// The index.md contains valid TOML frontmatter with an `id`, `title`, and `text` field.
/// doc1.md is a simple markdown document with a heading and a section.
#[allow(dead_code)]
pub fn create_test_network(temp_dir: &TempDir) -> PathBuf {
    let network_path = temp_dir.path().join("test_network");
    std::fs::create_dir(&network_path).unwrap();

    // Create index file with TOML frontmatter
    let network_index = r#"---
id = "test-network"
title = "Test Network"
text = "A test belief network"
---

# Test Network

A test belief network.
"#;
    std::fs::write(network_path.join("index.md"), network_index).unwrap();

    // Create a sample markdown document
    let doc1 = r#"# Document 1

This is a test document.

## Section 1

Some content here.
"#;
    std::fs::write(network_path.join("doc1.md"), doc1).unwrap();

    network_path
}
