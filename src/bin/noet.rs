//! noet CLI tool
//!
//! Command-line interface for parsing and watching markdown documents with noet-core.
//!
//! ## Commands
//!
//! - `parse <path>`: One-shot parsing with diagnostics
//! - `watch <path>`: Continuous file watching and parsing

use clap::{Parser, Subcommand};
use noet_core::{codec::parser::BeliefSetParser, event::Event, watch::WatchService};
use std::{path::PathBuf, sync::mpsc::channel, time::Duration};

#[derive(Parser)]
#[command(name = "noet")]
#[command(author, version, about = "A tool for parsing and watching markdown documents", long_about = None)]
struct Cli {
    #[command(subcommand)]
    command: Commands,
}

#[derive(Subcommand)]
enum Commands {
    /// Parse a document or directory once and display diagnostics
    Parse {
        /// Path to the document or directory to parse
        path: PathBuf,

        /// Verbose output
        #[arg(short, long)]
        verbose: bool,
    },

    /// Watch a directory for changes and continuously parse
    Watch {
        /// Path to the directory to watch
        path: PathBuf,

        /// Verbose output
        #[arg(short, long)]
        verbose: bool,

        /// Configuration file path
        #[arg(short, long)]
        config: Option<PathBuf>,
    },
}

fn main() -> Result<(), Box<dyn std::error::Error>> {
    // Initialize tracing
    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| tracing_subscriber::EnvFilter::new("info")),
        )
        .init();

    let cli = Cli::parse();

    match cli.command {
        Commands::Parse { path, verbose } => {
            if verbose {
                println!("Parsing: {:?}", path);
            }

            // Create a simple parser without event transmission
            let parser = BeliefSetParser::simple(&path)?;

            // Parse all documents
            let runtime = tokio::runtime::Builder::new_current_thread()
                .enable_all()
                .build()?;
            runtime.block_on(async {
                let mut parser = parser;
                // Use the accumulated BeliefSet as cache for parsing
                let cache = parser.accumulator().set().clone();
                parser.parse_all(cache).await?;

                // Get final stats
                let stats = parser.stats();

                println!("\n=== Parse Results ===");
                println!("Primary queue: {}", stats.primary_queue_len);
                println!("Reparse queue: {}", stats.reparse_queue_len);
                println!("Processed: {}", stats.processed_count);
                println!("Total parses: {}", stats.total_parses);
                println!("Pending dependencies: {}", stats.pending_dependencies_count);

                // TODO: Display diagnostics from accumulated cache
                // For now, we just show that parsing completed

                Ok::<(), noet_core::BuildonomyError>(())
            })?;

            if verbose {
                println!("\nParsing completed successfully");
            }

            Ok(())
        }

        Commands::Watch {
            path,
            verbose,
            config,
        } => {
            if verbose {
                println!("Watching: {:?}", path);
                if let Some(ref cfg) = config {
                    println!("Config: {:?}", cfg);
                }
            }

            // Determine root directory for service
            let root_dir = if let Some(cfg_path) = config {
                cfg_path
                    .parent()
                    .map(|p| p.to_path_buf())
                    .unwrap_or_else(|| std::env::current_dir().unwrap())
            } else {
                std::env::current_dir()?
            };

            // Create event channel
            let (tx, rx) = channel::<Event>();

            // Spawn event handler thread
            let event_handle = std::thread::spawn(move || {
                for event in rx {
                    println!("[Event] {:?}", event);
                }
            });

            // Create watch service
            let service = WatchService::new(root_dir, tx)?;

            // Enable network syncer for the path
            service.enable_network_syncer(&path)?;

            println!(
                "Watching {} for changes. Press Ctrl-C to stop.",
                path.display()
            );

            // Set up Ctrl-C handler
            let running = std::sync::Arc::new(std::sync::atomic::AtomicBool::new(true));
            let r = running.clone();

            ctrlc::set_handler(move || {
                println!("\nShutting down...");
                r.store(false, std::sync::atomic::Ordering::SeqCst);
            })?;

            // Keep running until Ctrl-C
            while running.load(std::sync::atomic::Ordering::SeqCst) {
                std::thread::sleep(Duration::from_millis(100));
            }

            // Cleanup
            service.disable_network_syncer(&path)?;
            drop(service);
            drop(event_handle);

            println!("Shutdown complete");

            Ok(())
        }
    }
}
