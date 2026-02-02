//! noet CLI tool
//!
//! Command-line interface for parsing and watching markdown documents with noet-core.
//!
//! ## Commands
//!
//! - `parse <path>`: One-shot parsing with diagnostics
//! - `watch <path>`: Continuous file watching and parsing
//!
//! ## Write-Back Support
//!
//! By default, both commands operate in read-only mode. Use the `--write` flag to enable
//! writing normalized/updated content back to source files.
//!
//! **Warning**: The `--write` flag modifies files in place. Ensure you have backups or are
//! using version control before enabling write-back.
//!
//! ### Write-Back Implementation Details
//!
//! **Parse command**: Writes all modified files after parsing completes. Uses atomic write
//! operations (temp file + rename) to prevent partial writes on failure.
//!
//! **Watch command**: Writes files immediately after each parse. To prevent re-parse loops,
//! the file watcher uses path-specific ignoring:
//! - After writing a file, adds it to an ignore set for 3 seconds
//! - File system events for ignored paths are filtered out by the debouncer
//! - After 3 seconds, the path is removed from the ignore set
//! - This allows the compiler's own writes to be ignored while detecting legitimate user edits
//!   to other files immediately

// Compile-time check for service feature (required for watch command)
#[cfg(all(not(feature = "service"), not(doc)))]
compile_error!(
    "The 'watch' subcommand requires the 'service' feature. \
     Please rebuild with '--features service' or use the default 'bin' feature."
);

use clap::{Parser, Subcommand};
#[cfg(feature = "service")]
mod dev_server;
#[cfg(feature = "service")]
use noet_core::watch::WatchService;
use noet_core::{codec::compiler::DocumentCompiler, event::Event};
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

        /// Write normalized/updated content back to source files (default: read-only)
        #[arg(short, long)]
        write: bool,

        /// Force re-parse all files, ignoring cache
        #[arg(long)]
        force: bool,

        /// Optional output directory for HTML generation
        #[arg(long)]
        html_output: Option<PathBuf>,
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

        /// Write normalized/updated content back to source files (default: read-only).
        /// The watch service ignores its own writes for 3 seconds to prevent re-parse loops.
        #[arg(short, long)]
        write: bool,

        /// Optional output directory for HTML generation
        #[arg(long)]
        html_output: Option<PathBuf>,

        /// Start HTTP server for viewing HTML output (requires --html-output)
        #[arg(long)]
        serve: bool,

        /// Port for dev server (default: 9037)
        #[arg(long, default_value = "9037")]
        port: u16,
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
        Commands::Parse {
            path,
            verbose,
            write,
            force,
            html_output,
        } => {
            if verbose {
                println!("Parsing: {path:?}");
                if write {
                    println!("Write-back: ENABLED (files will be modified)");
                } else {
                    println!("Write-back: disabled (read-only mode)");
                }
            }

            // Create a compiler with write flag and optional HTML output
            let compiler = if let Some(ref html_dir) = html_output {
                std::fs::create_dir_all(html_dir)?;
                DocumentCompiler::with_html_output(
                    &path,
                    None,
                    None,
                    write,
                    Some(html_dir.clone()),
                    None, // No live reload script for parse command
                )?
            } else {
                DocumentCompiler::new(&path, None, None, write)?
            };

            // Parse all documents
            let runtime = tokio::runtime::Builder::new_current_thread()
                .enable_all()
                .build()?;
            runtime.block_on(async {
                let mut compiler = compiler;
                // Use the accumulated BeliefBase as cache for parsing
                let cache = compiler.builder().doc_bb().clone();
                compiler.parse_all(cache, force).await?;

                // Get final stats
                let stats = compiler.stats();

                println!("\n=== Parse Results ===");
                println!("Primary queue: {}", stats.primary_queue_len);
                println!("Reparse queue: {}", stats.reparse_queue_len);
                println!("Processed: {}", stats.processed_count);
                println!("Total parses: {}", stats.total_parses);
                println!("Pending dependencies: {}", stats.pending_dependencies_count);

                if write {
                    println!("\n=== Write Results ===");
                    println!("Files processed: {}", stats.processed_count);
                    println!("Note: Only modified files are written back");
                }

                // Complete HTML generation if output directory specified
                if let Some(ref html_dir) = html_output {
                    // Generate network index pages
                    if let Err(e) = compiler.generate_network_indices().await {
                        eprintln!("Warning: Failed to generate network indices: {}", e);
                    }

                    // Create asset hardlinks
                    // Get asset manifest from session_bb (contains all parsed content)
                    use noet_core::properties::asset_namespace;
                    use noet_core::query::BeliefSource;
                    let asset_manifest: std::collections::BTreeMap<
                        String,
                        noet_core::properties::Bid,
                    > = compiler
                        .builder()
                        .session_bb()
                        .get_network_paths(asset_namespace())
                        .await
                        .unwrap_or_default()
                        .into_iter()
                        .filter(|(path, _)| !path.is_empty()) // Skip network node itself
                        .collect();

                    if let Err(e) = compiler
                        .create_asset_hardlinks(html_dir, &asset_manifest)
                        .await
                    {
                        eprintln!("Warning: Failed to create asset hardlinks: {}", e);
                    }

                    println!("\n=== HTML Export Results ===");
                    println!("HTML output: {}", html_dir.display());
                    println!("Asset hardlinks: {} assets", asset_manifest.len());
                }

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
            write,
            html_output,
            serve,
            port,
        } => {
            #[cfg(not(feature = "service"))]
            {
                eprintln!("Error: The 'watch' subcommand requires the 'service' feature.");
                eprintln!("Please rebuild with: cargo build --features service");
                std::process::exit(1);
            }

            #[cfg(feature = "service")]
            {
                // Validate: --serve requires --html-output
                if serve && html_output.is_none() {
                    eprintln!("Error: --serve requires --html-output to be specified");
                    std::process::exit(1);
                }

                if verbose {
                    println!("Watching: {path:?}");
                    if let Some(ref cfg) = config {
                        println!("Config: {cfg:?}");
                    }
                    if write {
                        println!("Write-back: ENABLED (files will be modified on change)");
                    } else {
                        println!("Write-back: disabled (read-only mode)");
                    }
                    if let Some(ref html_dir) = html_output {
                        println!("HTML output: {}", html_dir.display());
                    }
                    if serve {
                        println!("Dev server: enabled on port {}", port);
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

                // Spawn event handler thread with write support
                let event_verbose = verbose;
                let event_handle = std::thread::spawn(move || {
                    for event in rx {
                        if event_verbose {
                            println!("[Event] {event:?}");
                        }
                    }
                });

                // Build live reload script if serving
                let live_reload_script = if serve {
                    Some(
                        r#"
<script>
(function() {
    'use strict';

    console.log('[noet] Connecting to dev server...');

    const eventSource = new EventSource('/events');

    eventSource.addEventListener('reload', function(e) {
        console.log('[noet] File change detected, reloading...');
        window.location.reload();
    });

    eventSource.addEventListener('close', function(e) {
        console.log('[noet] Server shutting down, closing connection...');
        eventSource.close();
    });

    eventSource.addEventListener('open', function(e) {
        console.log('[noet] Connected to dev server');
    });

    eventSource.addEventListener('error', function(e) {
        if (e.target.readyState === EventSource.CLOSED) {
            console.log('[noet] Connection closed');
        } else if (e.target.readyState === EventSource.CONNECTING) {
            console.log('[noet] Reconnecting...');
        } else {
            console.error('[noet] Connection error:', e);
        }
    });

    // Clean up on page unload
    window.addEventListener('beforeunload', function() {
        eventSource.close();
    });
})();
</script>"#
                            .to_string(),
                    )
                } else {
                    None
                };

                // Create watch service with write flag and optional HTML output
                let service = if let Some(ref html_dir) = html_output {
                    std::fs::create_dir_all(html_dir)?;
                    WatchService::with_html_output(
                        root_dir.clone(),
                        tx,
                        write,
                        Some(html_dir.clone()),
                        live_reload_script,
                    )?
                } else {
                    WatchService::new(root_dir.clone(), tx, write)?
                };

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

                // Start dev server if --serve flag is set
                let server_handle = if serve {
                    let html_dir = html_output.clone().unwrap(); // Safe: validated above
                    let running_clone = running.clone();

                    Some(std::thread::spawn(move || {
                        let rt = tokio::runtime::Builder::new_current_thread()
                            .enable_all()
                            .build()
                            .expect("Failed to create tokio runtime for dev server");

                        rt.block_on(async {
                            let dev_server = dev_server::DevServer::new(html_dir, port);

                            // Shutdown signal based on running flag
                            let shutdown = async move {
                                while running_clone.load(std::sync::atomic::Ordering::SeqCst) {
                                    tokio::time::sleep(Duration::from_millis(100)).await;
                                }
                            };

                            if let Err(e) = dev_server.serve(shutdown).await {
                                eprintln!("Dev server error: {}", e);
                            }
                        });
                    }))
                } else {
                    None
                };

                // Keep running until Ctrl-C
                while running.load(std::sync::atomic::Ordering::SeqCst) {
                    std::thread::sleep(Duration::from_millis(100));
                }

                // Cleanup
                service.disable_network_syncer(&path)?;
                drop(service);
                drop(event_handle);

                if let Some(handle) = server_handle {
                    // Try to join with timeout - if it doesn't complete in 3 seconds, just move on
                    // The thread will be orphaned but the process is exiting anyway
                    let join_result = std::thread::spawn(move || handle.join());

                    let timeout_duration = Duration::from_secs(3);
                    let start = std::time::Instant::now();

                    loop {
                        if start.elapsed() > timeout_duration {
                            eprintln!("Warning: Dev server shutdown timed out after 3s");
                            break;
                        }

                        if join_result.is_finished() {
                            break;
                        }

                        std::thread::sleep(Duration::from_millis(100));
                    }
                }

                println!("Shutdown complete");

                Ok(())
            }
        }
    }
}
