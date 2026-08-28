//! Asset tracking and content addressing tests.

#![cfg(feature = "service")]

use noet_core::{beliefbase::BeliefBase, codec::DocumentCompiler, event::BeliefEvent};
use test_log::test;
use tokio::sync::mpsc::unbounded_channel;

use super::common::generate_test_root;

/// Compile `network_1` to HTML in a temp dir.
///
/// Returns `(source_tmp, html_tmp)`. Both must stay alive for the duration of
/// the assertions — dropping a `TempDir` deletes the tree.
async fn compile_network_1_to_html() -> (tempfile::TempDir, tempfile::TempDir) {
    let (src_tmp, test_root) = generate_test_root("network_1").unwrap();
    let html_tmp = tempfile::tempdir().unwrap();

    let (tx, mut rx) = unbounded_channel::<BeliefEvent>();

    let mut event_bb = BeliefBase::empty();
    let processor = tokio::spawn(async move {
        while let Some(event) = rx.recv().await {
            let _ = event_bb.process_event(&event);
        }
        event_bb
    });

    let mut compiler = DocumentCompiler::with_html_output(
        &test_root,
        Some(tx),
        Some(5),
        false,
        Some(html_tmp.path().to_path_buf()),
        None,
        false,
        None,
        None,
        false,
    )
    .unwrap();

    let cache = compiler.cache().clone();
    compiler.parse_all(cache, false).await.unwrap();

    compiler.builder_mut().close_tx();
    let final_bb = processor.await.unwrap();

    compiler.finalize_html(&final_bb).await.unwrap();

    (src_tmp, html_tmp)
}

/// Binary assets must not be copied into `pages/sources/`.
///
/// `create_asset_hardlinks` already emits every asset twice: content-addressed
/// at `static/{sha256}.{ext}`, and at its semantic path under `pages/` as a
/// hardlink to that same inode. `copy_source_files` previously copied every
/// entry in `latest_results` indiscriminately, adding a third — and this one a
/// real byte-for-byte copy, not a hardlink.
///
/// On an asset-heavy corpus that third copy dominated the output (23.6 GB of a
/// 40 GB site), so this is a size regression guard, not a cosmetic one.
#[test(tokio::test)]
async fn assets_are_not_duplicated_into_pages_sources() {
    let (_src_tmp, html_tmp) = compile_network_1_to_html().await;
    let site = html_tmp.path();

    // The asset must still be served from its semantic path under pages/.
    let semantic = site.join("pages/assets/test_image.png");
    assert!(
        semantic.exists(),
        "asset should be served at pages/assets/test_image.png; \
         found nothing at {}",
        semantic.display(),
    );

    // ...and must NOT have been copied under pages/sources/.
    let sources_copy = site.join("pages/sources/assets/test_image.png");
    assert!(
        !sources_copy.exists(),
        "asset was duplicated into pages/sources/ — copy_source_files is not \
         skipping AssetCodec results. This silently multiplies output size on \
         asset-heavy corpora.",
    );

    // Belt and braces: no binary asset of any kind under pages/sources/.
    let sources_dir = site.join("pages/sources");
    if sources_dir.exists() {
        let mut offenders = Vec::new();
        let mut stack = vec![sources_dir.clone()];
        while let Some(dir) = stack.pop() {
            let Ok(entries) = std::fs::read_dir(&dir) else {
                continue;
            };
            for entry in entries.flatten() {
                let path = entry.path();
                if path.is_dir() {
                    stack.push(path);
                } else if matches!(
                    path.extension().and_then(|e| e.to_str()),
                    Some("png" | "jpg" | "jpeg" | "gif" | "pdf" | "xlsx")
                ) {
                    offenders.push(path);
                }
            }
        }
        assert!(
            offenders.is_empty(),
            "binary assets found under pages/sources/: {offenders:?}",
        );
    }
}

/// Markdown sources *should* still be copied to `pages/sources/`.
///
/// Guards the opposite direction: the asset fix must not suppress the feature
/// `copy_source_files` exists for — giving readers a downloadable copy of the
/// document source alongside the rendered HTML.
#[test(tokio::test)]
async fn markdown_sources_are_still_copied() {
    let (_src_tmp, html_tmp) = compile_network_1_to_html().await;
    let site = html_tmp.path();

    let sources_dir = site.join("pages/sources");
    assert!(
        sources_dir.exists(),
        "pages/sources/ should exist and hold markdown sources",
    );

    let mut md_count = 0usize;
    let mut stack = vec![sources_dir.clone()];
    while let Some(dir) = stack.pop() {
        let Ok(entries) = std::fs::read_dir(&dir) else {
            continue;
        };
        for entry in entries.flatten() {
            let path = entry.path();
            if path.is_dir() {
                stack.push(path);
            } else if path.extension().and_then(|e| e.to_str()) == Some("md") {
                md_count += 1;
            }
        }
    }

    assert!(
        md_count > 0,
        "expected at least one .md source under pages/sources/, found none — \
         the asset skip is too aggressive",
    );
}

/// The semantic path under `pages/` and the canonical file under `static/`
/// must share one inode, so serving both costs the bytes only once.
///
/// If this ever falls back to a plain copy, output size doubles silently.
#[cfg(unix)]
#[test(tokio::test)]
async fn semantic_asset_path_is_hardlinked_to_canonical() {
    use std::os::unix::fs::MetadataExt;

    let (_src_tmp, html_tmp) = compile_network_1_to_html().await;
    let site = html_tmp.path();

    let semantic = site.join("pages/assets/test_image.png");
    assert!(semantic.exists(), "semantic asset path missing");

    let static_dir = site.join("static");
    assert!(static_dir.exists(), "static/ should hold canonical assets");

    let semantic_meta = std::fs::metadata(&semantic).unwrap();

    // Find the canonical file with the same inode.
    let shares_inode = std::fs::read_dir(&static_dir)
        .unwrap()
        .flatten()
        .filter(|e| e.path().is_file())
        .any(|e| {
            std::fs::metadata(e.path())
                .map(|m| m.ino() == semantic_meta.ino())
                .unwrap_or(false)
        });

    assert!(
        shares_inode,
        "pages/assets/test_image.png does not share an inode with any file in \
         static/ — the hardlink fell back to a copy, doubling asset bytes",
    );
}
