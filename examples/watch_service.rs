//! Complete WatchService orchestration example
//!
//! This example demonstrates how to:
//! - Initialize a WatchService with custom configuration
//! - Enable file watching for multiple document networks
//! - Process events in real-time
//! - Query the graph state
//! - Manage network configuration
//! - Handle graceful shutdown
//!
//! Run this example with:
//! ```bash
//! cargo run --features service --example watch_service -- /path/to/workspace
//! ```

#[cfg(feature = "service")]
use noet_core::{
    config::NetworkRecord,
    event::{BeliefEvent, Event},
    properties::{BeliefNode, Bid},
    watch::WatchService,
};
#[cfg(feature = "service")]
use std::{
    env,
    path::PathBuf,
    sync::mpsc::{channel, TryRecvError},
    thread,
    time::Duration,
};

/// Example: Basic WatchService with single network
#[cfg(feature = "service")]
fn example_basic_watch(workspace_root: PathBuf) -> Result<(), Box<dyn std::error::Error>> {
    println!("\n=== Example 1: Basic Watch Service ===\n");

    // Create event channel
    let (tx, rx) = channel::<Event>();

    // Initialize service (creates database at workspace_root/belief_cache.db)
    println!("Initializing WatchService at: {}", workspace_root.display());
    let service = WatchService::new(workspace_root.clone(), tx, true)?;

    // Enable watching for a network
    let network_path = workspace_root.join("docs");
    if network_path.exists() {
        println!("Enabling file watcher for: {}", network_path.display());
        service.enable_network_syncer(&network_path)?;

        // Wait for initial parse
        println!("Waiting for initial parse...");
        thread::sleep(Duration::from_secs(2));

        // Process events
        println!("Processing events (Ctrl-C to stop)...\n");
        let mut event_count = 0;
        for _ in 0..10 {
            match rx.try_recv() {
                Ok(Event::Belief(belief_event)) => {
                    event_count += 1;
                    println!("  [{event_count}] Received belief event: {belief_event}");
                }
                Ok(Event::Focus(_)) => {
                    println!("  Received focus event");
                }
                Ok(Event::Ping) => {
                    // Keepalive, ignore
                }
                Err(TryRecvError::Empty) => {
                    thread::sleep(Duration::from_millis(100));
                }
                Err(TryRecvError::Disconnected) => {
                    println!("Event channel disconnected");
                    break;
                }
            }
        }

        println!("\nReceived {event_count} events during initial parse");

        // Disable watcher before shutdown
        println!("Disabling network syncer...");
        service.disable_network_syncer(&network_path)?;
    } else {
        println!("Network path does not exist: {}", network_path.display());
        println!("Create a directory with documents to watch");
    }

    println!("Example 1 complete!\n");
    Ok(())
}

/// Example: Multiple networks with configuration
#[cfg(feature = "service")]
fn example_multiple_networks(workspace_root: PathBuf) -> Result<(), Box<dyn std::error::Error>> {
    println!("\n=== Example 2: Multiple Networks with Configuration ===\n");

    let (tx, _rx) = channel::<Event>();
    let service = WatchService::new(workspace_root.clone(), tx, true)?;

    // Get current networks (reads from config.toml)
    let networks = service.get_networks()?;
    println!("Currently configured networks: {}", networks.len());
    for (i, net) in networks.iter().enumerate() {
        println!("  {}. {} ({})", i + 1, net.node.title, net.path);
    }

    // Add a new network to configuration
    let new_network_path = workspace_root.join("new_network");
    let mut updated_networks = networks;
    updated_networks.push(NetworkRecord {
        path: new_network_path.to_string_lossy().to_string(),
        node: BeliefNode {
            bid: Bid::nil(),
            kind: Default::default(),
            title: "New Network Example".to_string(),
            schema: None,
            payload: Default::default(),
            id: Some("new-network-example".to_string()),
        },
    });

    // Save updated configuration
    println!("\nAdding new network to configuration...");
    let saved_networks = service.set_networks(Some(updated_networks))?;
    println!(
        "Configuration saved. Total networks: {}",
        saved_networks.len()
    );

    // Configuration is persisted to workspace_root/config.toml
    let config_path = workspace_root.join("config.toml");
    if config_path.exists() {
        println!("Configuration persisted to: {}", config_path.display());
    }

    println!("Example 2 complete!\n");
    Ok(())
}

/// Example: Event processing with detailed logging
#[cfg(feature = "service")]
fn example_event_processing(workspace_root: PathBuf) -> Result<(), Box<dyn std::error::Error>> {
    println!("\n=== Example 3: Detailed Event Processing ===\n");

    let (tx, rx) = channel::<Event>();
    let service = WatchService::new(workspace_root.clone(), tx, true)?;

    let network_path = workspace_root.join("docs");
    if !network_path.exists() {
        println!("Creating example network at: {}", network_path.display());
        std::fs::create_dir_all(&network_path)?;

        // Create index.md file
        std::fs::write(
            network_path.join("index.md"),
            r#"---
id: "example-network"
title: "Example Network"
---

# Example Network

This is an example network for testing the watch service.
"#,
        )?;

        // Create sample document
        std::fs::write(
            network_path.join("sample.md"),
            r#"# Sample Document

This is an example document for testing file watching.

## Section 1

Some content here.
"#,
        )?;

        println!("Created example network with sample document");
    }

    service.enable_network_syncer(&network_path)?;
    println!("Watching for changes in: {}\n", network_path.display());

    // Process events for 5 seconds
    println!("Processing events for 5 seconds...");
    let start = std::time::Instant::now();
    let mut stats = EventStats::default();

    while start.elapsed() < Duration::from_secs(5) {
        match rx.try_recv() {
            Ok(Event::Belief(belief_event)) => {
                process_belief_event(&belief_event, &mut stats);
            }
            Ok(Event::Focus(focus_event)) => {
                println!("  [Focus] {focus_event:?}");
                stats.focus_events += 1;
            }
            Ok(Event::Ping) => {
                stats.ping_events += 1;
            }
            Err(TryRecvError::Empty) => {
                thread::sleep(Duration::from_millis(100));
            }
            Err(TryRecvError::Disconnected) => {
                println!("Event channel disconnected");
                break;
            }
        }
    }

    // Print statistics
    println!("\n--- Event Statistics ---");
    println!("Node updates: {}", stats.node_updates);
    println!("Nodes removed: {}", stats.nodes_removed);
    println!("Paths added: {}", stats.paths_added);
    println!("Relations updated: {}", stats.relations_updated);
    println!("Focus events: {}", stats.focus_events);
    println!("Ping events: {}", stats.ping_events);
    println!("Total events: {}", stats.total());

    service.disable_network_syncer(&network_path)?;
    println!("\nExample 3 complete!\n");
    Ok(())
}

/// Example: Long-running service with graceful shutdown
#[cfg(feature = "service")]
fn example_long_running(workspace_root: PathBuf) -> Result<(), Box<dyn std::error::Error>> {
    println!("\n=== Example 4: Long-Running Service ===\n");
    println!("This example runs until Ctrl-C is pressed\n");

    let (tx, rx) = channel::<Event>();
    let service = WatchService::new(workspace_root.clone(), tx, true)?;

    let network_path = workspace_root.join("docs");
    if !network_path.exists() {
        println!("No network found at: {}", network_path.display());
        println!("Skipping long-running example");
        return Ok(());
    }

    service.enable_network_syncer(&network_path)?;
    println!("Watching: {}", network_path.display());
    println!("Modify files in this directory to see events");
    println!("Press Ctrl-C to stop\n");

    // Set up Ctrl-C handler
    let running = std::sync::Arc::new(std::sync::atomic::AtomicBool::new(true));
    let r = running.clone();
    ctrlc::set_handler(move || {
        println!("\nReceived Ctrl-C, shutting down...");
        r.store(false, std::sync::atomic::Ordering::SeqCst);
    })
    .expect("Error setting Ctrl-C handler");

    // Process events until interrupted
    let mut event_count = 0;
    while running.load(std::sync::atomic::Ordering::SeqCst) {
        match rx.try_recv() {
            Ok(Event::Belief(belief_event)) => {
                event_count += 1;
                println!("[{event_count}] {belief_event}");
            }
            Ok(Event::Focus(_)) => {
                println!("[Focus event]");
            }
            Ok(Event::Ping) => {
                // Keepalive
            }
            Err(TryRecvError::Empty) => {
                thread::sleep(Duration::from_millis(100));
            }
            Err(TryRecvError::Disconnected) => {
                println!("Event channel disconnected");
                break;
            }
        }
    }

    // Graceful shutdown
    println!("\nShutting down gracefully...");
    service.disable_network_syncer(&network_path)?;
    println!("Service stopped. Processed {event_count} events");

    println!("Example 4 complete!\n");
    Ok(())
}

// Helper structures and functions

#[derive(Default)]
#[cfg(feature = "service")]
struct EventStats {
    node_updates: usize,
    nodes_removed: usize,
    paths_added: usize,
    relations_updated: usize,
    focus_events: usize,
    ping_events: usize,
}

#[cfg(feature = "service")]
impl EventStats {
    fn total(&self) -> usize {
        self.node_updates
            + self.nodes_removed
            + self.paths_added
            + self.relations_updated
            + self.focus_events
            + self.ping_events
    }
}

#[cfg(feature = "service")]
fn process_belief_event(event: &BeliefEvent, stats: &mut EventStats) {
    match event {
        BeliefEvent::NodeUpdate(keys, _toml, origin) => {
            stats.node_updates += 1;
            println!(
                "  [NodeUpdate] {} node(s), origin: {:?}",
                keys.len(),
                origin
            );
        }
        BeliefEvent::NodesRemoved(bids, origin) => {
            stats.nodes_removed += 1;
            println!(
                "  [NodesRemoved] {} node(s), origin: {:?}",
                bids.len(),
                origin
            );
        }
        BeliefEvent::NodeRenamed(from, to, origin) => {
            println!("  [NodeRenamed] {from} -> {to}, origin: {origin:?}");
        }
        BeliefEvent::PathAdded(network, path, _node, _order, origin) => {
            stats.paths_added += 1;
            println!("  [PathAdded] {path} in network {network}, origin: {origin:?}");
        }
        BeliefEvent::PathUpdate(network, path, _node, _order, origin) => {
            println!("  [PathUpdate] {path} in network {network}, origin: {origin:?}");
        }
        BeliefEvent::PathsRemoved(network, paths, origin) => {
            println!(
                "  [PathsRemoved] {} path(s) from network {}, origin: {:?}",
                paths.len(),
                network,
                origin
            );
        }
        BeliefEvent::RelationUpdate(source, sink, weights, origin) => {
            stats.relations_updated += 1;
            println!(
                "  [RelationUpdate] {} -> {}, {} weight(s), origin: {:?}",
                source,
                sink,
                weights.weights.len(),
                origin
            );
        }
        BeliefEvent::RelationChange(source, sink, kind, _weight, origin) => {
            println!("  [RelationChange] {source} -> {sink} ({kind:?}), origin: {origin:?}");
        }
        BeliefEvent::RelationRemoved(source, sink, origin) => {
            println!("  [RelationRemoved] {source} -> {sink}, origin: {origin:?}");
        }
        BeliefEvent::BalanceCheck => {
            println!("  [BalanceCheck]");
        }
        BeliefEvent::FileParsed(path) => {
            println!("  [FileParse] {:?}", path);
        }
        BeliefEvent::BuiltInTest => {
            println!("  [BuiltInTest]");
        }
    }
}

#[cfg(feature = "service")]
fn main() -> Result<(), Box<dyn std::error::Error>> {
    // Get workspace root from command line or use current directory
    let workspace_root = env::args()
        .nth(1)
        .map(PathBuf::from)
        .unwrap_or_else(|| env::current_dir().expect("Failed to get current directory"));

    println!("╔══════════════════════════════════════════════════════════════╗");
    println!("║        WatchService Complete Orchestration Example          ║");
    println!("╚══════════════════════════════════════════════════════════════╝");
    println!("\nWorkspace: {}\n", workspace_root.display());

    // Run examples
    println!("This example demonstrates four usage patterns:\n");
    println!("1. Basic file watching with event processing");
    println!("2. Multiple networks with persistent configuration");
    println!("3. Detailed event logging and statistics");
    println!("4. Long-running service with graceful shutdown");
    println!("\nRunning examples...\n");

    // Example 1: Basic watch
    if let Err(e) = example_basic_watch(workspace_root.clone()) {
        eprintln!("Example 1 failed: {e}");
    }

    // Example 2: Multiple networks
    if let Err(e) = example_multiple_networks(workspace_root.clone()) {
        eprintln!("Example 2 failed: {e}");
    }

    // Example 3: Event processing
    if let Err(e) = example_event_processing(workspace_root.clone()) {
        eprintln!("Example 3 failed: {e}");
    }

    // Example 4: Long-running (optional, requires Ctrl-C)
    println!("Run example 4? (long-running until Ctrl-C) [y/N]: ");
    let mut input = String::new();
    std::io::stdin().read_line(&mut input)?;
    if input.trim().to_lowercase() == "y" {
        if let Err(e) = example_long_running(workspace_root.clone()) {
            eprintln!("Example 4 failed: {e}");
        }
    } else {
        println!("Skipping example 4");
    }

    println!("\n╔══════════════════════════════════════════════════════════════╗");
    println!("║                    All Examples Complete                     ║");
    println!("╚══════════════════════════════════════════════════════════════╝\n");

    Ok(())
}

#[cfg(not(feature = "service"))]
fn main() {}
