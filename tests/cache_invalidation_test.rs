//! Cache invalidation integration tests
//!
//! These tests verify that file modification time tracking works correctly
//! with the full WatchService setup including database synchronization.

#[cfg(feature = "service")]
use filetime::{set_file_mtime, FileTime};
#[cfg(feature = "service")]
use noet_core::{
    db::{db_init, DbConnection},
    event::Event,
    watch::WatchService,
};
#[cfg(feature = "service")]
use std::{path::PathBuf, sync::mpsc::channel, time::Duration};
#[cfg(feature = "service")]
use tempfile::TempDir;

/// Helper to create a test network with index.md file
#[cfg(feature = "service")]
fn create_test_network(temp_dir: &TempDir) -> PathBuf {
    let network_path = temp_dir.path().join("test_network");
    std::fs::create_dir(&network_path).unwrap();

    // Create index.md file
    let network_content = r#"---
id: "test-network"
title: "Test Network"
text: "A test belief network for cache invalidation testing"
---

# Test Network

A test belief network for cache invalidation testing.
"#;
    std::fs::write(network_path.join("index.md"), network_content).unwrap();

    network_path
}

#[test]
#[cfg(feature = "service")]
fn test_mtime_tracking() {
    // Test that file mtimes are tracked in the database after parsing
    let temp_dir = TempDir::new().unwrap();
    let root_dir = temp_dir.path().to_path_buf();
    let network_path = create_test_network(&temp_dir);

    // Create a test document
    let doc_path = network_path.join("test.md");
    std::fs::write(&doc_path, "# Test Document\n\nHello world!").unwrap();

    let (tx, _rx) = channel::<Event>();

    // Create WatchService with database
    let service = WatchService::new(root_dir.clone(), tx, false).unwrap();

    // Enable network syncer
    service.enable_network_syncer(&network_path).unwrap();

    // Wait for initial parse and transaction processing
    std::thread::sleep(Duration::from_secs(3));

    // Check that mtime was tracked in database
    let rt = tokio::runtime::Runtime::new().unwrap();
    rt.block_on(async {
        let db_path = root_dir.join("belief_cache.db");
        let pool = db_init(db_path).await.unwrap();
        let db = DbConnection(pool);

        let mtimes = db.get_file_mtimes().await.unwrap();

        // Verify test.md has a tracked mtime
        let doc_mtime = mtimes
            .iter()
            .find(|(path, _)| path.to_string_lossy().contains("test.md"));

        assert!(
            doc_mtime.is_some(),
            "test.md should have mtime tracked. Found mtimes: {:?}",
            mtimes
        );

        let (_path, mtime) = doc_mtime.unwrap();
        assert!(*mtime > 0, "Cached mtime should be positive");
    });

    // Cleanup
    service.disable_network_syncer(&network_path).ok();
}

#[test]
#[cfg(feature = "service")]
fn test_stale_file_detection_and_reparse() {
    // Test that modified files are detected as stale and re-parsed
    let temp_dir = TempDir::new().unwrap();
    let root_dir = temp_dir.path().to_path_buf();
    let network_path = create_test_network(&temp_dir);

    // Create a test document
    let doc_path = network_path.join("test.md");
    std::fs::write(&doc_path, "# Original Content\n").unwrap();

    let (tx, rx) = channel::<Event>();

    // Create WatchService
    let service = WatchService::new(root_dir.clone(), tx, false).unwrap();

    // Enable network syncer
    service.enable_network_syncer(&network_path).unwrap();

    // Wait for initial parse
    std::thread::sleep(Duration::from_secs(3));

    // Get initial mtime
    let rt = tokio::runtime::Runtime::new().unwrap();
    let initial_mtime = rt.block_on(async {
        let db_path = root_dir.join("belief_cache.db");
        let pool = db_init(db_path).await.unwrap();
        let db = DbConnection(pool);

        let mtimes = db.get_file_mtimes().await.unwrap();
        let doc_mtime = mtimes
            .iter()
            .find(|(path, _)| path.to_string_lossy().contains("test.md"));

        assert!(doc_mtime.is_some(), "test.md should be tracked initially");
        *doc_mtime.unwrap().1
    });

    // Drain initial events
    while rx.try_recv().is_ok() {}

    // Wait to ensure mtime will be different
    // Windows NTFS has 2-second resolution for write times (per Microsoft docs)
    std::thread::sleep(Duration::from_secs(3));

    // Modify the file
    std::fs::write(&doc_path, "# Modified Content\n\nThis is new!").unwrap();

    // Explicitly set mtime to current time + 60 seconds to ensure it's different
    // Use FileTime::now() instead of unix timestamp arithmetic to avoid NTFS rounding issues
    // The file watcher has a 2-second debounce, so this happens before the reparse
    let current_time = FileTime::now();
    let future_mtime = FileTime::from_unix_time(current_time.unix_seconds() + 60, 0);
    set_file_mtime(&doc_path, future_mtime).unwrap();

    // Wait for file watcher to detect change, debounce, and reparse
    std::thread::sleep(Duration::from_secs(7));

    // Verify we received events (indicating reparse happened)
    let mut event_count = 0;
    while rx.try_recv().is_ok() {
        event_count += 1;
    }

    assert!(
        event_count > 0,
        "Expected to receive events after file modification (reparse happened)"
    );

    // Check that mtime was updated
    let new_mtime = rt.block_on(async {
        let db_path = root_dir.join("belief_cache.db");
        let pool = db_init(db_path).await.unwrap();
        let db = DbConnection(pool);

        let mtimes = db.get_file_mtimes().await.unwrap();
        let doc_mtime = mtimes
            .iter()
            .find(|(path, _)| path.to_string_lossy().contains("test.md"));

        assert!(doc_mtime.is_some(), "test.md should still be tracked");
        *doc_mtime.unwrap().1
    });

    assert!(
        new_mtime > initial_mtime,
        "Mtime should be updated after reparse: old={}, new={}",
        initial_mtime,
        new_mtime
    );

    // Cleanup
    service.disable_network_syncer(&network_path).ok();
}

#[test]
#[cfg(feature = "service")]
fn test_multiple_files_mtime_tracking() {
    // Test that mtimes are tracked for multiple files
    let temp_dir = TempDir::new().unwrap();
    let root_dir = temp_dir.path().to_path_buf();
    let network_path = create_test_network(&temp_dir);

    // Create multiple test documents
    std::fs::write(network_path.join("doc1.md"), "# Document 1\n").unwrap();
    std::fs::write(network_path.join("doc2.md"), "# Document 2\n").unwrap();
    std::fs::write(network_path.join("doc3.md"), "# Document 3\n").unwrap();

    let (tx, _rx) = channel::<Event>();

    // Create WatchService
    let service = WatchService::new(root_dir.clone(), tx, false).unwrap();

    // Enable network syncer
    service.enable_network_syncer(&network_path).unwrap();

    // Wait for initial parse
    std::thread::sleep(Duration::from_secs(4));

    // Check that all files have mtimes tracked
    let rt = tokio::runtime::Runtime::new().unwrap();
    rt.block_on(async {
        let db_path = root_dir.join("belief_cache.db");
        let pool = db_init(db_path).await.unwrap();
        let db = DbConnection(pool);

        let mtimes = db.get_file_mtimes().await.unwrap();

        let doc1 = mtimes
            .iter()
            .any(|(path, _)| path.to_string_lossy().contains("doc1.md"));
        let doc2 = mtimes
            .iter()
            .any(|(path, _)| path.to_string_lossy().contains("doc2.md"));
        let doc3 = mtimes
            .iter()
            .any(|(path, _)| path.to_string_lossy().contains("doc3.md"));

        assert!(doc1, "doc1.md should have mtime tracked");
        assert!(doc2, "doc2.md should have mtime tracked");
        assert!(doc3, "doc3.md should have mtime tracked");
    });

    // Cleanup
    service.disable_network_syncer(&network_path).ok();
}

#[test]
#[cfg(feature = "service")]
fn test_deleted_file_handling() {
    // Test that deleted files are handled gracefully
    let temp_dir = TempDir::new().unwrap();
    let root_dir = temp_dir.path().to_path_buf();
    let network_path = create_test_network(&temp_dir);

    // Create a test document
    let doc_path = network_path.join("to_delete.md");
    std::fs::write(&doc_path, "# To Be Deleted\n").unwrap();

    let (tx, _rx) = channel::<Event>();

    // Create WatchService
    let service = WatchService::new(root_dir.clone(), tx, false).unwrap();

    // Enable network syncer
    service.enable_network_syncer(&network_path).unwrap();

    // Wait for initial parse
    std::thread::sleep(Duration::from_secs(3));

    // Verify file was tracked
    let rt = tokio::runtime::Runtime::new().unwrap();
    let was_tracked = rt.block_on(async {
        let db_path = root_dir.join("belief_cache.db");
        let pool = db_init(db_path).await.unwrap();
        let db = DbConnection(pool);

        let mtimes = db.get_file_mtimes().await.unwrap();
        mtimes
            .iter()
            .any(|(path, _)| path.to_string_lossy().contains("to_delete.md"))
    });

    assert!(was_tracked, "File should be tracked before deletion");

    // Delete the file
    std::fs::remove_file(&doc_path).unwrap();

    // Wait for file watcher to detect deletion and process
    std::thread::sleep(Duration::from_secs(7));

    // Service should handle this gracefully without panicking
    // The test passes if we get here without crashes

    // Cleanup
    service.disable_network_syncer(&network_path).ok();
}

#[test]
#[cfg(feature = "service")]
fn test_unchanged_files_keep_same_mtime() {
    // Test that unchanged files don't have their mtime updated unnecessarily
    let temp_dir = TempDir::new().unwrap();
    let root_dir = temp_dir.path().to_path_buf();
    let network_path = create_test_network(&temp_dir);

    // Create a test document
    let doc_path = network_path.join("unchanged.md");
    std::fs::write(&doc_path, "# Unchanged Document\n").unwrap();

    let (tx, _rx) = channel::<Event>();

    // Create WatchService
    let service = WatchService::new(root_dir.clone(), tx, false).unwrap();

    // Enable network syncer
    service.enable_network_syncer(&network_path).unwrap();

    // Wait for initial parse
    std::thread::sleep(Duration::from_secs(3));

    // Get initial mtime
    let rt = tokio::runtime::Runtime::new().unwrap();
    let initial_mtime = rt.block_on(async {
        let db_path = root_dir.join("belief_cache.db");
        let pool = db_init(db_path).await.unwrap();
        let db = DbConnection(pool);

        let mtimes = db.get_file_mtimes().await.unwrap();
        let doc_mtime = mtimes
            .iter()
            .find(|(path, _)| path.to_string_lossy().contains("unchanged.md"));

        assert!(doc_mtime.is_some(), "unchanged.md should be tracked");
        *doc_mtime.unwrap().1
    });

    // Disable and re-enable syncer to simulate a restart
    service.disable_network_syncer(&network_path).ok();
    std::thread::sleep(Duration::from_millis(500));
    service.enable_network_syncer(&network_path).unwrap();

    // Wait for second parse
    std::thread::sleep(Duration::from_secs(3));

    // Check that mtime is unchanged (file wasn't modified)
    let new_mtime = rt.block_on(async {
        let db_path = root_dir.join("belief_cache.db");
        let pool = db_init(db_path).await.unwrap();
        let db = DbConnection(pool);

        let mtimes = db.get_file_mtimes().await.unwrap();
        let doc_mtime = mtimes
            .iter()
            .find(|(path, _)| path.to_string_lossy().contains("unchanged.md"));

        assert!(doc_mtime.is_some(), "unchanged.md should still be tracked");
        *doc_mtime.unwrap().1
    });

    assert_eq!(
        initial_mtime, new_mtime,
        "Mtime should be unchanged for unmodified file"
    );

    // Cleanup
    service.disable_network_syncer(&network_path).ok();
}
