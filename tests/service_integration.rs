//! Integration tests for WatchService (file watching, parsing, database sync)
//!
//! These tests verify end-to-end behavior using the public API:
//! - WatchService initialization and configuration
//! - File watching with automatic reparsing
//! - Database synchronization
//! - Service lifecycle (enable/disable watchers, shutdown)
//!
//! Tests focus on observable behavior rather than internal implementation details.

#[cfg(feature = "service")]
use noet_core::{
    config::NetworkRecord,
    event::Event,
    properties::{BeliefNode, Bid},
    watch::WatchService,
};
#[cfg(feature = "service")]
use std::{path::PathBuf, sync::mpsc::channel, time::Duration};
#[cfg(feature = "service")]
use tempfile::TempDir;

/// Helper to create a test directory with sample documents
/// Helper to create a test network with .noet file
#[cfg(feature = "service")]
fn create_test_network(temp_dir: &TempDir) -> PathBuf {
    let network_path = temp_dir.path().join("test_network");
    std::fs::create_dir(&network_path).unwrap();

    // Create .noet file
    let network_toml = r#"
id = "test-network"
title = "Test Network"
text = "A test belief network"
"#;
    std::fs::write(network_path.join(".noet"), network_toml).unwrap();

    // Create a sample markdown document
    let doc1 = r#"# Document 1

This is a test document.

## Section 1

Some content here.
"#;
    std::fs::write(network_path.join("doc1.md"), doc1).unwrap();

    network_path
}

#[test]
#[cfg(feature = "service")]
fn test_watch_service_initialization() {
    // Test that WatchService can be created and initializes correctly
    let temp_dir = TempDir::new().unwrap();
    let root_dir = temp_dir.path().to_path_buf();

    let (tx, _rx) = channel::<Event>();

    // Create WatchService - it creates its own runtime and db internally
    let service = WatchService::new(root_dir, tx, false);

    // Service should initialize successfully (this is just a compile/construction test)
    assert!(
        service.is_ok(),
        "WatchService should initialize successfully"
    );
}

#[test]
#[cfg(feature = "service")]
fn test_watch_service_enable_disable_network_syncer() {
    // Test enabling and disabling network syncer
    let temp_dir = TempDir::new().unwrap();
    let root_dir = temp_dir.path().to_path_buf();
    let network_path = create_test_network(&temp_dir);

    let (tx, _rx) = channel::<Event>();

    let service = WatchService::new(root_dir, tx, false).unwrap();

    // Enable network syncer
    let enable_result = service.enable_network_syncer(&network_path);
    assert!(
        enable_result.is_ok(),
        "Should successfully enable network syncer: {:?}",
        enable_result.err()
    );

    // Wait for initial parse to complete
    std::thread::sleep(Duration::from_secs(3));

    // Disable network syncer
    let disable_result = service.disable_network_syncer(&network_path);
    assert!(
        disable_result.is_ok(),
        "Should successfully disable network syncer: {:?}",
        disable_result.err()
    );
}

#[test]
#[cfg(feature = "service")]
fn test_file_modification_triggers_reparse() {
    // Test that modifying a file triggers automatic reparsing
    // Note: This test can be flaky due to file system notification timing
    let temp_dir = TempDir::new().unwrap();
    let root_dir = temp_dir.path().to_path_buf();
    let network_path = create_test_network(&temp_dir);
    let doc_path = network_path.join("doc1.md");

    let (tx, rx) = channel::<Event>();

    let service = WatchService::new(root_dir, tx, false).unwrap();

    // Enable network syncer
    service.enable_network_syncer(&network_path).unwrap();

    // Wait for initial parse
    std::thread::sleep(Duration::from_secs(3));

    // Drain initial events
    while rx.try_recv().is_ok() {}

    // Modify the document
    let updated_content = r#"# Updated Document

This content has changed.

## New Section

With new content.
"#;
    std::fs::write(&doc_path, updated_content).unwrap();

    // Wait for file watcher debouncer and reparse
    // Note: File system notification timing varies by OS and load
    std::thread::sleep(Duration::from_secs(7));

    // Verify we received events (indicating reparse happened)
    let mut event_count = 0;
    while rx.try_recv().is_ok() {
        event_count += 1;
    }

    // Note: With debounce (2s) + processing time, 7s sleep should be sufficient
    // If this fails, check logs for debouncer/compiler activity
    assert!(
        event_count > 0,
        "Expected to receive events after file modification, got {event_count}. \
         This may be a timing issue in the test environment."
    );

    // Cleanup
    service.disable_network_syncer(&network_path).ok();
}

#[test]
#[cfg(feature = "service")]
fn test_multiple_file_changes_processed() {
    // Test that multiple file changes are all processed
    let temp_dir = TempDir::new().unwrap();
    let root_dir = temp_dir.path().to_path_buf();
    let network_path = create_test_network(&temp_dir);

    let (tx, _rx) = channel::<Event>();

    let service = WatchService::new(root_dir, tx, false).unwrap();

    // Enable network syncer
    service.enable_network_syncer(&network_path).unwrap();

    // Wait for initial parse
    std::thread::sleep(Duration::from_secs(3));

    // Create multiple new documents
    for i in 2..5 {
        let doc_content = format!("# Document {i}\n\nContent for document {i}.");
        std::fs::write(network_path.join(format!("doc{i}.md")), doc_content).unwrap();
    }

    // Wait for processing (debouncer + parse time)
    std::thread::sleep(Duration::from_secs(6));

    // If we got here without panics, the service handled multiple changes
    // More detailed verification would require querying the database or compiler stats

    // Cleanup
    service.disable_network_syncer(&network_path).ok();
}

#[test]
#[cfg(feature = "service")]
fn test_service_handles_empty_files() {
    // Test that empty files don't crash the service
    let temp_dir = TempDir::new().unwrap();
    let root_dir = temp_dir.path().to_path_buf();
    let network_path = create_test_network(&temp_dir);

    // Create an empty file
    std::fs::write(network_path.join("empty.md"), "").unwrap();

    let (tx, _rx) = channel::<Event>();

    let service = WatchService::new(root_dir, tx, false).unwrap();

    // Enable network syncer
    service.enable_network_syncer(&network_path).unwrap();

    // Wait for processing
    std::thread::sleep(Duration::from_secs(3));

    // If we got here, the service handled the empty file gracefully
    // No panics or crashes expected

    // Cleanup
    service.disable_network_syncer(&network_path).ok();
}

#[test]
#[cfg(feature = "service")]
fn test_shutdown_cleanup() {
    // Test that WatchService cleans up properly when dropped
    let temp_dir = TempDir::new().unwrap();
    let root_dir = temp_dir.path().to_path_buf();
    let network_path = create_test_network(&temp_dir);

    let (tx, _rx) = channel::<Event>();

    {
        let service = WatchService::new(root_dir, tx, false).unwrap();

        // Enable network syncer
        service.enable_network_syncer(&network_path).unwrap();

        // Wait a bit
        std::thread::sleep(Duration::from_secs(2));

        // Service will be dropped here
    }

    // Wait for cleanup
    std::thread::sleep(Duration::from_millis(500));

    // If we got here without panics or hangs, cleanup worked
    // The file watcher threads should have been aborted when service was dropped
}

#[test]
#[cfg(feature = "service")]
fn test_get_set_networks() {
    // Test get_networks and set_networks operations
    let temp_dir = TempDir::new().unwrap();
    let root_dir = temp_dir.path().to_path_buf();
    let network_path = create_test_network(&temp_dir);

    let (tx, _rx) = channel::<Event>();

    let service = WatchService::new(root_dir, tx, false).unwrap();

    // Initially no networks
    let networks = service.get_networks().unwrap();
    assert_eq!(networks.len(), 0, "Should start with no networks");

    // Set a network (set_networks returns the updated list)
    let node = BeliefNode {
        bid: Bid::new(Bid::nil()),
        kind: Default::default(),
        title: "Test Network".to_string(),
        schema: None,
        payload: Default::default(),
        id: Some("test-network".to_string()),
    };
    let record = NetworkRecord {
        path: network_path.to_string_lossy().to_string(),
        node,
    };
    let updated_networks = service.set_networks(Some(vec![record.clone()])).unwrap();

    // Verify network is set
    assert_eq!(updated_networks.len(), 1);
    assert_eq!(updated_networks[0].path, record.path);

    // Verify get_networks returns the same
    let networks = service.get_networks().unwrap();
    assert_eq!(networks.len(), 1);
    assert_eq!(networks[0].path, record.path);
}

#[test]
#[cfg(feature = "service")]
fn test_database_connection_is_public() {
    // Test that DbConnection can be constructed publicly (for custom database paths)
    use noet_core::db::{db_init, DbConnection};

    let temp_dir = TempDir::new().unwrap();
    let db_path = temp_dir.path().join("custom.db");

    // Create a current_thread runtime (no multi-thread needed for this test)
    let runtime = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .unwrap();
    let pool = runtime.block_on(db_init(db_path)).unwrap();

    // This should compile - DbConnection constructor is public
    let _db_conn = DbConnection(pool);
}
