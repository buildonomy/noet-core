//! Integration tests for service functionality (file watching, parsing, database sync)
//!
//! These tests verify the end-to-end behavior of:
//! - FileUpdateSyncer continuous parsing
//! - File watching with debouncing
//! - Database synchronization via perform_transaction
//! - Service orchestration (LatticeService/NoetService/WatchService)

use noet_core::{
    codec::CodecMap,
    config::NetworkRecord,
    db::{db_init, DbConnection},
    event::Event,
    query::{BeliefCache, PaginatedQuery, Query},
};
use std::{
    path::PathBuf,
    sync::mpsc::{channel, Sender},
    time::Duration,
};
use tempfile::TempDir;
use tokio::time::sleep;

/// Helper to create a test directory with sample documents
fn create_test_network(temp_dir: &TempDir) -> PathBuf {
    let network_path = temp_dir.path().join("test_network");
    std::fs::create_dir(&network_path).unwrap();

    // Create BeliefNetwork.toml
    let network_toml = r#"
id = "test-network"
title = "Test Network"
text = "A test belief network"
"#;
    std::fs::write(network_path.join("BeliefNetwork.toml"), network_toml).unwrap();

    // Create a sample markdown document
    let doc1 = r#"# Document 1

This is a test document.

## Section 1

Some content here.
"#;
    std::fs::write(network_path.join("doc1.md"), doc1).unwrap();

    network_path
}

/// Helper to create an event channel for testing
fn test_event_channel() -> Sender<Event> {
    let (tx, _rx) = channel();
    tx
}

#[tokio::test]
#[ignore = "TODO: Implement after renaming compiler.rs to service.rs"]
async fn test_file_update_syncer_initialization() {
    // Test that FileUpdateSyncer can be created and initializes correctly
    let temp_dir = TempDir::new().unwrap();
    let network_path = create_test_network(&temp_dir);

    let runtime = tokio::runtime::Runtime::new().unwrap();
    let db_path = temp_dir.path().join("test.db");
    let db = runtime.block_on(db_init(db_path)).unwrap();
    let db_conn = DbConnection(db);

    let codecs = CodecMap::create();
    let tx = test_event_channel();

    // TODO: Update to use new module name after rename
    // let syncer = FileUpdateSyncer::new(
    //     codecs,
    //     &db_conn,
    //     &tx,
    //     &network_path,
    //     true,
    //     &runtime,
    // ).unwrap();

    // Verify syncer has spawned threads
    // assert!(!syncer.parser_handle.is_finished());
    // assert!(!syncer.transaction_handle.is_finished());

    // Cleanup
    // syncer.parser_handle.abort();
    // syncer.transaction_handle.abort();
}

#[tokio::test]
#[ignore = "TODO: Implement after renaming compiler.rs to service.rs"]
async fn test_file_update_syncer_processes_queue() {
    // Test that FileUpdateSyncer processes documents in its queue
    let temp_dir = TempDir::new().unwrap();
    let network_path = create_test_network(&temp_dir);

    let runtime = tokio::runtime::Runtime::new().unwrap();
    let db_path = temp_dir.path().join("test.db");
    let db = runtime.block_on(db_init(db_path)).unwrap();
    let db_conn = DbConnection(db);

    let codecs = CodecMap::create();
    let tx = test_event_channel();

    // TODO: Create syncer and wait for initial parse
    // let syncer = FileUpdateSyncer::new(...);

    // Wait for parser to process initial queue
    sleep(Duration::from_secs(3)).await;

    // Verify documents were parsed and added to database
    // let cache = db_conn.clone();
    // let states = cache.get_states(...).await.unwrap();
    // assert!(!states.states.is_empty(), "Expected documents to be parsed and cached");

    // Cleanup
    // syncer.parser_handle.abort();
    // syncer.transaction_handle.abort();
}

#[tokio::test]
#[ignore = "TODO: Implement after renaming compiler.rs to service.rs"]
async fn test_file_modification_triggers_reparse() {
    // Test that modifying a file triggers re-parsing
    let temp_dir = TempDir::new().unwrap();
    let network_path = create_test_network(&temp_dir);
    let doc_path = network_path.join("doc1.md");

    let runtime = tokio::runtime::Runtime::new().unwrap();
    let db_path = temp_dir.path().join("test.db");
    let db = runtime.block_on(db_init(db_path)).unwrap();
    let db_conn = DbConnection(db);

    let codecs = CodecMap::create();
    let tx = test_event_channel();

    // TODO: Create syncer
    // let syncer = FileUpdateSyncer::new(...);

    // Wait for initial parse
    sleep(Duration::from_secs(3)).await;

    // Modify the document
    let updated_content = r#"# Updated Document

This content has changed.
"#;
    std::fs::write(&doc_path, updated_content).unwrap();

    // Wait for debouncer and reparse
    sleep(Duration::from_secs(4)).await;

    // Verify updated content is in database
    // TODO: Query database to verify changes were synced

    // Cleanup
    // syncer.parser_handle.abort();
    // syncer.transaction_handle.abort();
}

#[tokio::test]
#[ignore = "TODO: Implement after renaming compiler.rs to service.rs"]
async fn test_multiple_file_changes_processed() {
    // Test that multiple file changes are all processed
    let temp_dir = TempDir::new().unwrap();
    let network_path = create_test_network(&temp_dir);

    let runtime = tokio::runtime::Runtime::new().unwrap();
    let db_path = temp_dir.path().join("test.db");
    let db = runtime.block_on(db_init(db_path)).unwrap();
    let db_conn = DbConnection(db);

    let codecs = CodecMap::create();
    let tx = test_event_channel();

    // TODO: Create syncer
    // let syncer = FileUpdateSyncer::new(...);

    // Wait for initial parse
    sleep(Duration::from_secs(3)).await;

    // Create multiple new documents
    for i in 2..5 {
        let doc_content = format!("# Document {}\n\nContent for document {}.", i, i);
        std::fs::write(network_path.join(format!("doc{}.md", i)), doc_content).unwrap();
    }

    // Wait for processing
    sleep(Duration::from_secs(5)).await;

    // Verify all documents were processed
    // TODO: Check parser stats or database to verify all files processed

    // Cleanup
    // syncer.parser_handle.abort();
    // syncer.transaction_handle.abort();
}

#[tokio::test]
#[ignore = "TODO: Implement after renaming compiler.rs to service.rs"]
async fn test_parse_errors_handled_gracefully() {
    // Test that parse errors don't crash the syncer
    let temp_dir = TempDir::new().unwrap();
    let network_path = create_test_network(&temp_dir);

    // Create a document that might cause parse issues (empty file)
    std::fs::write(network_path.join("empty.md"), "").unwrap();

    let runtime = tokio::runtime::Runtime::new().unwrap();
    let db_path = temp_dir.path().join("test.db");
    let db = runtime.block_on(db_init(db_path)).unwrap();
    let db_conn = DbConnection(db);

    let codecs = CodecMap::create();
    let tx = test_event_channel();

    // TODO: Create syncer
    // let syncer = FileUpdateSyncer::new(...);

    // Wait for processing
    sleep(Duration::from_secs(3)).await;

    // Verify syncer is still running (didn't crash)
    // assert!(!syncer.parser_handle.is_finished());
    // assert!(!syncer.transaction_handle.is_finished());

    // Cleanup
    // syncer.parser_handle.abort();
    // syncer.transaction_handle.abort();
}

#[tokio::test]
#[ignore = "TODO: Implement after renaming compiler.rs to service.rs"]
async fn test_syncer_shutdown_cleanup() {
    // Test that FileUpdateSyncer cleans up properly on shutdown
    let temp_dir = TempDir::new().unwrap();
    let network_path = create_test_network(&temp_dir);

    let runtime = tokio::runtime::Runtime::new().unwrap();
    let db_path = temp_dir.path().join("test.db");
    let db = runtime.block_on(db_init(db_path)).unwrap();
    let db_conn = DbConnection(db);

    let codecs = CodecMap::create();
    let tx = test_event_channel();

    // TODO: Create syncer
    // let syncer = FileUpdateSyncer::new(...);

    // Wait a bit
    sleep(Duration::from_secs(2)).await;

    // Abort threads
    // syncer.parser_handle.abort();
    // syncer.transaction_handle.abort();

    // Wait for cleanup
    sleep(Duration::from_millis(500)).await;

    // Verify threads are stopped
    // assert!(syncer.parser_handle.is_finished());
    // assert!(syncer.transaction_handle.is_finished());
}

#[tokio::test]
#[ignore = "TODO: Implement after renaming and creating Service type"]
async fn test_service_initialization() {
    // Test that LatticeService/NoetService/WatchService initializes correctly
    let temp_dir = TempDir::new().unwrap();
    let root_dir = temp_dir.path().to_path_buf();
    let tx = test_event_channel();

    // TODO: Update to use renamed service type
    // let service = LatticeService::new(root_dir, tx).unwrap();

    // Verify service is initialized
    // assert!(service.get_networks().is_ok());
}

#[tokio::test]
#[ignore = "TODO: Implement after renaming and creating Service type"]
async fn test_service_enable_disable_network_syncer() {
    // Test enabling and disabling network syncer
    let temp_dir = TempDir::new().unwrap();
    let root_dir = temp_dir.path().to_path_buf();
    let network_path = create_test_network(&temp_dir);
    let tx = test_event_channel();

    // TODO: Create service
    // let service = LatticeService::new(root_dir, tx).unwrap();

    // Enable syncer
    // service.enable_belief_network_syncer(&network_path).unwrap();

    // Wait for initial parse
    sleep(Duration::from_secs(3)).await;

    // Verify syncer is running (modify file and check it's processed)

    // Disable syncer
    // service.disable_belief_network_syncer(&network_path).unwrap();

    // Verify syncer stopped (modify file and check it's NOT processed)
}

#[tokio::test]
#[ignore = "TODO: Implement after renaming and creating Service type"]
async fn test_service_get_set_networks() {
    // Test get_networks and set_networks operations
    let temp_dir = TempDir::new().unwrap();
    let root_dir = temp_dir.path().to_path_buf();
    let network_path = create_test_network(&temp_dir);
    let tx = test_event_channel();

    // TODO: Create service
    // let service = LatticeService::new(root_dir, tx).unwrap();

    // Initially no networks
    // let networks = service.get_networks().unwrap();
    // assert_eq!(networks.len(), 0);

    // Set a network
    // let record = NetworkRecord {
    //     path: network_path.to_string_lossy().to_string(),
    //     node: ...
    // };
    // service.set_networks(Some(vec![record.clone()])).unwrap();

    // Verify network is set
    // let networks = service.get_networks().unwrap();
    // assert_eq!(networks.len(), 1);
    // assert_eq!(networks[0].path, record.path);
}

#[tokio::test]
#[ignore = "TODO: Implement after renaming and creating Service type"]
async fn test_service_query_states() {
    // Test querying graph state via get_states
    let temp_dir = TempDir::new().unwrap();
    let root_dir = temp_dir.path().to_path_buf();
    let network_path = create_test_network(&temp_dir);
    let tx = test_event_channel();

    // TODO: Create service and set up network
    // let service = LatticeService::new(root_dir, tx).unwrap();
    // service.enable_belief_network_syncer(&network_path).unwrap();

    // Wait for initial parse
    sleep(Duration::from_secs(3)).await;

    // Query states
    // let query = PaginatedQuery { ... };
    // let results = service.get_states(query).await.unwrap();

    // Verify results
    // assert!(!results.results.states.is_empty());
}

#[tokio::test]
#[ignore = "TODO: Implement after refining commands.rs"]
async fn test_database_synchronization() {
    // Test that perform_transaction correctly batches events and updates database
    let temp_dir = TempDir::new().unwrap();
    let network_path = create_test_network(&temp_dir);

    let runtime = tokio::runtime::Runtime::new().unwrap();
    let db_path = temp_dir.path().join("test.db");
    let db = runtime.block_on(db_init(db_path)).unwrap();
    let db_conn = DbConnection(db);

    // TODO: Create event channel with receiver
    // let (tx, rx) = unbounded_channel();

    // TODO: Send multiple BeliefEvents
    // tx.send(...).unwrap();

    // TODO: Call perform_transaction
    // perform_transaction(rx, db_conn, ...).await.unwrap();

    // Verify database was updated
    // let states = db_conn.get_states(...).await.unwrap();
    // assert!(states contains expected data);
}

#[tokio::test]
#[ignore = "TODO: Implement after refining commands.rs"]
async fn test_transaction_error_handling() {
    // Test that transaction errors are handled gracefully
    // TODO: Test with invalid events or database errors
}

#[tokio::test]
#[ignore = "TODO: Implement - test debouncer filtering"]
async fn test_debouncer_filters_dot_files() {
    // Test that file watcher ignores dot files (e.g., .git, .DS_Store)
    let temp_dir = TempDir::new().unwrap();
    let network_path = create_test_network(&temp_dir);

    // TODO: Set up service with file watching
    // Create a dot file
    std::fs::write(network_path.join(".hidden"), "secret").unwrap();

    // Wait for debouncer
    sleep(Duration::from_secs(3)).await;

    // Verify dot file was NOT processed
    // (check parser stats or database to confirm)
}

#[tokio::test]
#[ignore = "TODO: Implement - test debouncer filtering"]
async fn test_debouncer_filters_by_codec_extensions() {
    // Test that file watcher only processes files with valid codec extensions
    let temp_dir = TempDir::new().unwrap();
    let network_path = create_test_network(&temp_dir);

    // TODO: Set up service with file watching
    // Create files with various extensions
    std::fs::write(network_path.join("test.txt"), "not markdown").unwrap();
    std::fs::write(network_path.join("test.jpg"), "image data").unwrap();

    // Wait for debouncer
    sleep(Duration::from_secs(3)).await;

    // Verify only .md files were processed
}

#[tokio::test]
#[ignore = "TODO: Implement - test thread synchronization"]
async fn test_no_race_conditions_between_debouncer_and_parser() {
    // Test that concurrent file changes don't cause race conditions
    let temp_dir = TempDir::new().unwrap();
    let network_path = create_test_network(&temp_dir);

    // TODO: Set up service
    // Rapidly modify multiple files
    // Verify all changes are processed correctly without deadlocks or panics
}
