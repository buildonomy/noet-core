//! Common utilities for codec tests

#![allow(dead_code)]

use noet_core::{beliefbase::BeliefBase, error::BuildonomyError, properties::Bid};
use petgraph::visit::{EdgeRef, IntoEdgeReferences};
use serde::Deserialize;
use std::{
    collections::BTreeSet,
    fs, io,
    path::{Path, PathBuf},
};

use tempfile::TempDir;
use toml::from_str;

/// Recursively copy `src` into `dst`, preserving symlinks.
///
/// Symlinks are recreated with the same target (relative or absolute) rather than
/// being followed and copied as regular files/directories.  This is necessary for
/// integration tests that exercise symlinked subnet directories: if symlinks were
/// silently skipped or dereferenced, the copy would not reflect the repo fixture.
pub fn copy_dir_all(src: impl AsRef<Path>, dst: impl AsRef<Path>) -> io::Result<()> {
    fs::create_dir_all(&dst)?;
    for entry in fs::read_dir(src)? {
        let entry = entry?;
        let ty = entry.file_type()?;
        if ty.is_symlink() {
            // Recreate the symlink with its original target so that relative
            // targets (e.g. `symlinked_subnet -> subnet2`) remain correct in
            // the temp directory.
            let link_target = fs::read_link(entry.path())?;
            let dst_link = dst.as_ref().join(entry.file_name());
            #[cfg(unix)]
            std::os::unix::fs::symlink(&link_target, &dst_link)?;
            #[cfg(windows)]
            {
                // On Windows, distinguish file vs directory symlinks.
                if link_target.is_dir() {
                    std::os::windows::fs::symlink_dir(&link_target, &dst_link)?;
                } else {
                    std::os::windows::fs::symlink_file(&link_target, &dst_link)?;
                }
            }
        } else if ty.is_dir() {
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

/// Collect BIDs for all nodes emitted by binary codecs (currently: xlsx/ods).
///
/// The `written_bids == cached_bids` assertion in `bid_tests` only applies to text
/// codecs, where every BID in the cache was written into a source file as
/// `bid = "uuid"` and can be extracted by `extract_bids_from_content`.
///
/// Binary codecs work differently. None of the xlsx-originated nodes flow through
/// `ParseResult::rewritten_content`:
///
/// - **Row nodes**: BIDs are written to the `__noet_bid__` hidden column in the xlsx
///   file via `generate_source_bytes()` when `write=true`, but the compiler flushes
///   them directly to disk — they never appear in `rewritten_content`.
/// - **Workbook node**: BID is written into the `bid` key of the schema YAML in cell
///   A1 of the `index` tab via `generate_source_bytes()` when `write=true`. Flushed
///   directly to disk — never appears in `rewritten_content`.
/// - **Tab container nodes**: BIDs are written into `tabs_meta.<tab_name>.bid` in the
///   schema YAML cell A1 via `generate_source_bytes()` when `write=true`. Also flushed
///   directly to disk — never appear in `rewritten_content`.
///
/// On the next parse (`write=false`), the stored BIDs are read back from the xlsx
/// file (row BIDs from `__noet_bid__` columns; workbook and tab BIDs from the schema
/// YAML in cell A1) and injected into the corresponding IRNodes so that
/// `speculative_path_key` takes the stable `NodeKey::Bid` path.
///
/// The correct approach for `bid_tests` is to exclude all xlsx-originated nodes from
/// the `written_bids == cached_bids` assertion and rely on the zero-graph-event
/// assertion on parse 2 to enforce BID stability end-to-end.
///
/// ## Detection
///
/// The xlsx codec injects codec-specific payload keys into every node it emits:
///
/// | Node kind       | `xlsx_tab` | `xlsx_row` |
/// |-----------------|------------|------------|
/// | Row node        | ✓          | ✓          |
/// | Tab container   | ✓          | –          |
/// | Workbook node   | –          | –          |
///
/// Row nodes and tab container nodes are detected directly by payload. The workbook
/// node carries no xlsx-specific payload — it is excluded separately because its
/// BID is also never written to disk (no `__noet_bid__` column on the workbook node
/// itself). We identify it as the direct `Document`-kind parent of a tab container:
/// one level of relation-graph expansion from the tab-container seed, filtered to
/// only include nodes that have `BeliefKind::Document` and are NOT themselves network
/// roots (network roots are `Kind::Network`, not `Kind::Document`). This avoids
/// pulling the enclosing markdown network node into the exclusion set.
pub fn all_xlsx_bids(global_bb: &BeliefBase) -> BTreeSet<Bid> {
    use noet_core::properties::BeliefKind;

    let asset_bids: BTreeSet<_> = global_bb
        .paths()
        .asset_map()
        .map()
        .iter()
        .map(|(_, bid, _)| *bid)
        .collect();

    let states = global_bb.states();

    // Seed: row nodes only — they carry both `xlsx_tab` and `xlsx_row` in payload,
    // injected unconditionally by `parse_tab`. Tab container nodes and the workbook
    // node carry neither key; they are found via two levels of relation-graph expansion.
    let seed_bids: BTreeSet<Bid> = states
        .values()
        .filter(|n| !asset_bids.contains(&n.bid))
        .filter(|n| n.payload.contains_key("xlsx_tab") && n.payload.contains_key("xlsx_row"))
        .map(|n| n.bid)
        .collect();

    if seed_bids.is_empty() {
        return seed_bids;
    }

    // Expand two levels up from the row-node seed:
    //
    //   Level 1: row nodes → tab container nodes (Symbol kind).
    //            Tab containers are direct Section-edge parents of row nodes.
    //            They carry no xlsx-specific payload so they are not in the seed.
    //
    //   Level 2: tab container nodes → workbook node (Document kind, non-network).
    //            The workbook node is the Section-edge parent of every tab container.
    //            It also carries no xlsx-specific payload.
    //
    // We must stop at level 2. The enclosing markdown network node is a Document-kind
    // parent of the workbook node (level 3). Including it would incorrectly exclude
    // text-codec BIDs that were written to markdown files from the cached_bids set.
    // Stopping condition: only add Symbol-kind nodes at level 1 and Document-kind
    // non-network nodes at level 2.
    let relations_guard = global_bb.relations();
    let graph = relations_guard.as_graph();

    // Level 1: row (seed) → tab container (Symbol, non-network).
    let mut level1 = seed_bids.clone();
    for edge_ref in graph.edge_references() {
        let source_bid = graph[edge_ref.source()];
        let sink_bid = graph[edge_ref.target()];
        if seed_bids.contains(&source_bid) && !asset_bids.contains(&sink_bid) {
            if let Some(parent) = states.get(&sink_bid) {
                if parent.kind.contains(BeliefKind::Symbol) && !parent.kind.is_network() {
                    level1.insert(sink_bid);
                }
            }
        }
    }

    // Level 2: tab container (level1 additions) → workbook node (Document, non-network).
    // The tab containers added at level 1 are in level1 but not in seed_bids.
    let tab_container_bids: BTreeSet<Bid> = level1.difference(&seed_bids).copied().collect();
    let mut expanded = level1;
    for edge_ref in graph.edge_references() {
        let source_bid = graph[edge_ref.source()];
        let sink_bid = graph[edge_ref.target()];
        if tab_container_bids.contains(&source_bid) && !asset_bids.contains(&sink_bid) {
            if let Some(parent) = states.get(&sink_bid) {
                if parent.kind.contains(BeliefKind::Document) && !parent.kind.is_network() {
                    expanded.insert(sink_bid);
                }
            }
        }
    }
    expanded
}
