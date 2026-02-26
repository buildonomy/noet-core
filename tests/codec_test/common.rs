//! Common utilities for codec tests

use noet_core::{error::BuildonomyError, properties::Bid};
use serde::Deserialize;
use std::{
    fs, io,
    path::{Path, PathBuf},
};
use tempfile::TempDir;
use toml::from_str;

pub fn copy_dir_all(src: impl AsRef<Path>, dst: impl AsRef<Path>) -> io::Result<()> {
    fs::create_dir_all(&dst)?;
    for entry in fs::read_dir(src)? {
        let entry = entry?;
        let ty = entry.file_type()?;
        if ty.is_dir() {
            copy_dir_all(entry.path(), dst.as_ref().join(entry.file_name()))?;
        } else {
            fs::copy(entry.path(), dst.as_ref().join(entry.file_name()))?;
        }
    }
    Ok(())
}

/// Create a temp directory and copy test network content into a named subdirectory.
///
/// Returns `(TempDir, PathBuf)` where the `PathBuf` is the content root
/// (e.g. `tempdir/network_1/`). The subdirectory preserves the network name so
/// that relative paths like `../network_1/assets/img.png` (parent-and-back
/// roundtrips) resolve correctly via the filesystem.
pub fn generate_test_root(test_net: &str) -> Result<(TempDir, PathBuf), BuildonomyError> {
    let temp_dir = tempfile::tempdir()?;
    tracing::debug!(
        "generating test root. Files: {}",
        fs::read_dir(&temp_dir)
            .unwrap()
            .map(|f| format!("{f:?}"))
            .collect::<Vec<String>>()
            .join(", ")
    );
    // Copy into a subdirectory named after the network so that relative paths
    // like `../network_1/assets/img.png` (parent-and-back roundtrips) resolve
    // correctly — the parent directory contains the named subdirectory.
    let test_root = temp_dir.path().join(test_net);
    let content_root = Path::new("tests").join(test_net);
    tracing::debug!("Copying content from {:?}", content_root);
    copy_dir_all(&content_root, &test_root)?;
    Ok((temp_dir, test_root))
}

#[derive(Debug, Default, Deserialize)]
struct ABid {
    bid: Bid,
}

/// Extracts Bids from lines matching the format "bid: 'uuid-string'"
pub fn extract_bids_from_content(content: &str) -> Result<Vec<Bid>, BuildonomyError> {
    let mut bids = Vec::new();
    for line in content.lines() {
        if line.trim().starts_with("bid") && line.trim()[3..].trim().starts_with('=') {
            let a_bid: ABid = from_str(line)?;
            bids.push(a_bid.bid);
        }
    }
    Ok(bids)
}
