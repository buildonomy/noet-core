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

use crate::codec::{compiler::DocumentCompiler, diagnostic::ParseDiagnostic};
#[cfg(feature = "service")]
use crate::event::Event;
#[cfg(feature = "service")]
use crate::watch::WatchService;
use clap::{Parser, Subcommand};
use std::io::IsTerminal;
use std::path::PathBuf;
#[cfg(feature = "service")]
use std::sync::mpsc::channel;
#[cfg(feature = "service")]
use std::time::Duration;

/// Write the full membership of the href and asset const-namespaces to `dump_path`
/// as TSV: `namespace<TAB>path<TAB>bid<TAB>classification`.
///
/// Diagnostic tooling for namespace-shape analysis.  `classification`
/// reports whether an entry's BID is *computable from its own key* or not:
///
/// - `derived_v5` — UUID v5, i.e. `buildonomy_href_bid(key)`.  The key alone
///   reproduces the BID, so no index is needed to resolve it.
/// - `timebased_v6` — UUID v6 from `Bid::new(namespace)`.  Carries the namespace
///   bref in octets 10-15 (so `parent_bref` matches) but is *not* reproducible
///   from the key; resolving it requires a stored path→BID mapping.
/// - `foreign` — BID belongs to another namespace entirely: a content node that
///   claimed this key via `url_aliases` / `alias-template`.
///
/// Note that `parent_bref` alone cannot distinguish the first two cases, since
/// both stamp the namespace bref into the same octets.  Only the UUID version
/// separates "computable" from "must be looked up".
///
/// Failures are logged and swallowed: this must never break a parse run.
async fn dump_const_namespaces<B: crate::query::BeliefSource>(source: &B, dump_path: &str) {
    use std::io::Write;

    /// Read the UUID version nibble from a BID's hyphenated string form — the
    /// first character of the third group. `Bid` wraps `Uuid` in a private field
    /// and exposes no version accessor; this is diagnostic-only code, so parse the
    /// display form rather than widen the public API for it.
    fn bid_uuid_version(bid: &crate::properties::Bid) -> Option<u32> {
        bid.to_string()
            .split('-')
            .nth(2)
            .and_then(|g| g.chars().next())
            .and_then(|c| c.to_digit(16))
    }

    let namespaces = [
        ("href", crate::properties::href_namespace()),
        ("asset", crate::properties::asset_namespace()),
    ];

    let mut out = String::from("namespace\tpath\tbid\tclassification\n");
    for (label, ns_bid) in namespaces {
        let entries = match source.submap(ns_bid, "", u8::MAX, true).await {
            Ok(entries) => entries,
            Err(e) => {
                tracing::warn!("[dump_const_namespaces] submap({label}) failed: {e}");
                continue;
            }
        };
        let ns_bref = ns_bid.bref();
        for (path, bid, _order) in entries {
            let classification = if bid == ns_bid {
                "namespace_root"
            } else if bid.parent_bref() != ns_bref {
                "foreign"
            } else if bid_uuid_version(&bid) == Some(5) {
                "derived_v5"
            } else {
                "timebased_v6"
            };
            out.push_str(&format!("{label}\t{path}\t{bid}\t{classification}\n"));
        }
    }

    match std::fs::File::create(dump_path).and_then(|mut f| f.write_all(out.as_bytes())) {
        Ok(()) => tracing::info!("[dump_const_namespaces] wrote {dump_path}"),
        Err(e) => tracing::warn!("[dump_const_namespaces] write to {dump_path} failed: {e}"),
    }
}

/// Color output mode for diagnostics.
#[derive(clap::ValueEnum, Clone, Default)]
pub enum ColorChoice {
    /// Emit color codes if stderr is a TTY, suppress them otherwise
    #[default]
    Auto,
    /// Always emit color codes (useful when piping through `less -R`)
    Always,
    /// Never emit color codes
    Never,
}

/// Top-level CLI definition.
#[derive(Parser)]
#[command(name = "noet")]
#[command(author, version, about = "A tool for parsing and watching markdown documents", long_about = None)]
pub struct Cli {
    /// Control color output: auto (default), always, never.
    /// `always` is useful when piping through a pager that supports ANSI escapes (e.g. `less -R`).
    /// Color is also suppressed when the NO_COLOR environment variable is set.
    #[arg(long, default_value = "auto", global = true)]
    pub color: ColorChoice,

    #[command(subcommand)]
    pub command: Commands,
}

/// Available CLI subcommands.
#[derive(Subcommand)]
pub enum Commands {
    /// Initialize a new network file with ID and title
    Init {
        /// Path where the network file should be created (default: current directory)
        #[arg(default_value = ".")]
        path: PathBuf,

        /// Network ID (will prompt if not provided)
        #[arg(long)]
        id: Option<String>,

        /// Network title (will prompt if not provided)
        #[arg(long)]
        title: Option<String>,

        /// Optional network summary
        #[arg(long)]
        summary: Option<String>,

        /// Insert a ````{network_children} placement marker in the body to control where the
        /// auto-generated child listing appears. If neither flag is passed, you will be prompted.
        #[arg(long, overrides_with = "no_children_marker")]
        children_marker: bool,

        /// Skip the ````{network_children} placement marker. If neither flag is passed, you
        /// will be prompted.
        #[arg(long, overrides_with = "children_marker")]
        no_children_marker: bool,
    },

    /// Print the bref (parent namespace reference) of a BID.
    ///
    /// A bref is the 6-byte parent-namespace fingerprint derived from a BID.
    /// It is displayed as a 12-character lowercase hex string.
    ///
    /// Example: noet bref 018f1234-abcd-6789-0000-b4a023772a74
    Bref {
        /// The BID to compute the bref of (hyphenated UUID format)
        bid: String,
    },

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

        /// Use CDN for Open Props (smaller output, requires internet)
        #[arg(long)]
        cdn: bool,

        /// Base URL for sitemap and canonical URLs (e.g., <https://username.github.io/repo>)
        /// Can also be set via NOET_BASE_URL environment variable
        #[arg(long)]
        base_url: Option<String>,

        /// Number of parallel jobs for epoch dispatch (default: available CPUs).
        /// Use 1 for sequential execution. Can also be set via NOET_JOBS env var.
        #[arg(short = 'j', long)]
        jobs: Option<usize>,

        /// Inject git repository metadata (commit, branch, dirty status, source backlinks)
        /// into BeliefNetwork nodes during parse. Can also be set via NOET_GIT_TRACKING=1.
        #[arg(long)]
        git_tracking: bool,

        /// Skip compile-time layout metadata (3D viewer render positions).
        /// Layout is on by default; pass this to omit it entirely.
        #[arg(long)]
        no_layout: bool,

        /// Skip layout for any network with more than N nodes. Layout cost is
        /// O(n^2) in a network's node count, so one oversized network can
        /// dominate the build. Can also be set via NOET_LAYOUT_MAX_NODES.
        #[arg(long, value_name = "N")]
        layout_max_nodes: Option<usize>,

        /// Disable the progress bar (useful when stdout/stderr is piped or redirected).
        #[arg(long)]
        no_progress: bool,

        /// Use a file-backed SQLite database at `<path>/belief_cache.db` instead
        /// of an ephemeral in-memory DB. The DB persists across runs; delete it
        /// manually for a fresh session. Can also be set via NOET_DB=1.
        #[arg(long)]
        db: bool,
    },

    /// Watch a directory for changes and continuously parse
    #[cfg(feature = "service")]
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

        /// Use CDN for Open Props (smaller output, requires internet)
        #[arg(long)]
        cdn: bool,

        /// Base URL for sitemap and canonical URLs (e.g., <https://username.github.io/repo>)
        /// Can also be set via NOET_BASE_URL environment variable
        #[arg(long)]
        base_url: Option<String>,

        /// Start HTTP server for viewing HTML output (requires --html-output)
        #[arg(long)]
        serve: bool,

        /// Port for dev server (default: 9037)
        #[arg(long, default_value = "9037")]
        port: u16,

        /// Inject git repository metadata (commit, branch, dirty status, source backlinks)
        /// into BeliefNetwork nodes during parse. Can also be set via NOET_GIT_TRACKING=1.
        #[arg(long)]
        git_tracking: bool,
    },

    /// Start an MCP (Model Context Protocol) server exposing BeliefBase query tools
    #[cfg(feature = "mcp")]
    Mcp {
        /// Load beliefbase from a pre-built output directory (static mode).
        /// Reads beliefbase/manifest.json and *.msgpack shards from this directory.
        /// Mutually exclusive with --watch.
        #[arg(long, conflicts_with = "watch")]
        output_dir: Option<PathBuf>,

        /// Watch a directory for changes and serve live BeliefBase queries via MCP (live mode).
        /// Uses the WatchService DbConnection as the query source — always reflects the latest
        /// compile pass without any in-memory rebuild or subscriber overhead.
        /// Mutually exclusive with --output-dir.
        #[arg(long, conflicts_with = "output_dir")]
        watch: Option<PathBuf>,

        /// Optional HTML output directory to use for search indices in live mode.
        /// Search indices are written by `noet parse --html-output`; if not provided,
        /// search returns empty results in live mode.
        #[arg(long, requires = "watch")]
        html_output: Option<PathBuf>,

        /// Inject git repository metadata into nodes during live-mode compile.
        #[arg(long, requires = "watch")]
        git_tracking: bool,
    },

    /// Package a rendered site for offline distribution to stakeholders
    #[cfg(feature = "distribute")]
    Distribute {
        /// Path to the rendered site (the --html-output directory from `noet parse`)
        site_path: PathBuf,

        /// Destination directory (default: <site_path>_dist)
        #[arg(long, short = 't')]
        target: Option<PathBuf>,

        /// Port number for generated launcher scripts
        #[arg(long, default_value = "8080")]
        port: u16,
    },
}

/// ANSI color codes for diagnostic output.
///
/// Respects `--color`, `is_terminal()`, and the `NO_COLOR` environment variable
/// (see <https://no-color.org>).
pub struct DiagColors {
    pub warning: &'static str,
    pub error: &'static str,
    pub info: &'static str,
    pub reset: &'static str,
}

impl DiagColors {
    pub fn new(choice: &ColorChoice) -> Self {
        let use_color = match choice {
            ColorChoice::Always => true,
            ColorChoice::Never => false,
            ColorChoice::Auto => {
                std::io::stderr().is_terminal() && std::env::var_os("NO_COLOR").is_none()
            }
        };
        if use_color {
            Self {
                warning: "\x1b[33m", // yellow
                error: "\x1b[31m",   // red
                info: "\x1b[36m",    // cyan
                reset: "\x1b[0m",
            }
        } else {
            Self {
                warning: "",
                error: "",
                info: "",
                reset: "",
            }
        }
    }
}

/// Entry point for the noet CLI.
///
/// Parses command-line arguments and dispatches to the appropriate subcommand.
/// This function contains the full CLI logic and can be called from any binary
/// crate that depends on `noet-core`.
pub fn run() -> Result<(), Box<dyn std::error::Error>> {
    // Initialize tracing
    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| tracing_subscriber::EnvFilter::new("info")),
        )
        .with_writer(std::io::stderr)
        .init();

    let cli = Cli::parse();
    let color_choice = cli.color.clone();

    match cli.command {
        Commands::Init {
            path,
            id,
            title,
            summary,
            children_marker,
            no_children_marker,
        } => {
            use std::io::Write;

            // Get ID - either from CLI or prompt
            let network_id = if let Some(id) = id {
                id
            } else {
                print!("Enter network ID: ");
                std::io::stdout().flush()?;
                let mut input = String::new();
                std::io::stdin().read_line(&mut input)?;
                input.trim().to_string()
            };

            if network_id.is_empty() {
                eprintln!("Error: Network ID cannot be empty");
                std::process::exit(1);
            }

            // Get title - either from CLI or prompt
            let network_title = if let Some(title) = title {
                Some(title)
            } else {
                print!("Enter network title: ");
                std::io::stdout().flush()?;
                let mut input = String::new();
                std::io::stdin().read_line(&mut input)?;
                let trimmed = input.trim().to_string();
                if trimmed.is_empty() {
                    None
                } else {
                    Some(trimmed)
                }
            };

            // Get summary if provided (no prompt if not on CLI)
            let network_summary = summary;

            // Determine whether to insert the network-children placement marker.
            // If neither --children-marker nor --no-children-marker was passed, prompt.
            let insert_children_marker = if children_marker {
                true
            } else if no_children_marker {
                false
            } else {
                print!("Insert child listing placement marker? [Y/n]: ");
                std::io::stdout().flush()?;
                let mut input = String::new();
                std::io::stdin().read_line(&mut input)?;
                let trimmed = input.trim().to_lowercase();
                // Empty input or 'y'/'yes' → true; anything else → false
                trimmed.is_empty() || trimmed == "y" || trimmed == "yes"
            };

            // Create network file
            let runtime = tokio::runtime::Builder::new_current_thread()
                .enable_all()
                .build()?;

            runtime.block_on(async {
                DocumentCompiler::create_network_file(
                    &path,
                    &network_id,
                    network_title,
                    network_summary,
                    insert_children_marker,
                )
                .await
            })?;

            // Network file is always named index.md
            let full_path = path.join("index.md");

            println!("✓ Network file created: {}", full_path.display());
            Ok(())
        }

        Commands::Bref { bid } => match crate::properties::Bid::try_from(bid.as_str()) {
            Ok(parsed_bid) => {
                println!("{}", parsed_bid.bref());
                Ok(())
            }
            Err(e) => {
                eprintln!("Error: invalid BID '{bid}': {e}");
                std::process::exit(1);
            }
        },

        Commands::Parse {
            path,
            verbose,
            write,
            force,
            html_output,
            cdn,
            base_url,
            jobs,
            git_tracking,
            no_layout,
            layout_max_nodes,
            no_progress,
            db,
        } => {
            // Read base_url from environment if not provided via CLI
            let base_url = base_url.or_else(|| std::env::var("NOET_BASE_URL").ok());
            // Resolve layout config: --no-layout disables; ceiling comes from
            // --layout-max-nodes, else NOET_LAYOUT_MAX_NODES, else the default.
            let layout_config = crate::layout::LayoutConfig {
                enabled: !no_layout,
                max_nodes: crate::layout::LayoutConfig::resolve_max_nodes(layout_max_nodes),
            };
            // Resolve git_tracking: CLI flag > NOET_GIT_TRACKING env var > false.
            let git_tracking = git_tracking
                || std::env::var("NOET_GIT_TRACKING")
                    .ok()
                    .map(|v| v == "1" || v.eq_ignore_ascii_case("true"))
                    .unwrap_or(false);
            // Resolve db: CLI flag > NOET_DB env var > false.
            let use_file_db = db
                || std::env::var("NOET_DB")
                    .ok()
                    .map(|v| v == "1" || v.eq_ignore_ascii_case("true"))
                    .unwrap_or(false);

            if verbose {
                println!("Parsing: {path:?}");
                if write {
                    println!("Write-back: ENABLED (files will be modified)");
                } else {
                    println!("Write-back: disabled (read-only mode)");
                }
            }

            // Parse all documents with explicit event loop management.
            //
            // Multi-threaded runtime (Issue 100 follow-up).  Under
            // `new_current_thread`, every `--jobs N` parse task multiplexes onto a
            // single OS thread.  `parse_content`'s Phase 2 is CPU-bound with no yield
            // points (there is no `spawn_blocking` anywhere in the parse path), so one
            // task starves all others — including tasks that only need to poll a
            // `Pool::acquire()` future.  That starvation surfaced as sqlx
            // "slow threshold" warnings that look like DB lock contention but are
            // actually the scheduler never polling the waiting task.
            //
            // Issue 100's `Mutex`→`RwLock` split was necessary but not sufficient:
            // it permits concurrent readers, but a single-threaded runtime cannot
            // execute them concurrently.  Note that Issue 100's regression test
            // `concurrent_evaluates_do_not_serialize` passes either way, because its
            // test source awaits `tokio::time::sleep` (a real yield point) rather than
            // blocking on CPU as production tasks do.
            //
            // Validated on a full large corpus run (`jobs=4`), against a
            // current_thread baseline of the same corpus and commit:
            //   wall clock        13.14h → 12.45h  (-5%)
            //   parse sequential  44.3h  → 34.8h   (-21%)
            //   slow-acquire      (unmeasured) → 0
            //   files/parses      64,473 / 67,378  (identical)
            //   warnings/errors   5,635 / 0        (identical)
            //
            // The identical file, parse, and warning counts are the correctness
            // evidence for running these tasks with real parallelism — they share
            // `global_bb` via `QueryHandle`, so a race would be expected to perturb
            // at least one of those totals.
            let runtime = tokio::runtime::Builder::new_multi_thread()
                .enable_all()
                .build()?;
            runtime.block_on(async {
                use crate::beliefbase::BeliefAccumulator;
                use crate::event::BeliefEvent;
                use tokio::sync::mpsc::unbounded_channel;

                // Create event channel for belief events.
                let (tx, rx) = unbounded_channel::<BeliefEvent>();

                // When the `service` feature is enabled, use an ephemeral in-memory SQLite
                // DB as the accumulator backing store.  `DbConnection::apply_batch` commits
                // all events for an epoch in a single SQL transaction, which is cheaper than
                // the `BeliefBase` path that drives `process_event` (including PathMapMap
                // reconstruction) for every event.  The DB is discarded at end of scope.
                //
                // Path events (PathAdded/PathUpdate/PathsRemoved) are harvested by
                // `terminate_stack` from `session_bb.process_event` derivatives and sent
                // on `tx`, so the `paths` table is correctly populated during parse.
                //
                // Without `service`, fall back to the in-memory `BeliefBase` accumulator.
                #[cfg(feature = "service")]
                let accumulator = {
                    use crate::db::{db_init, db_init_memory, DbConnection};
                    let db_pool = if use_file_db {
                        let db_path = path.join("belief_cache.db");
                        tracing::info!("Using file-backed belief DB: {}", db_path.display());
                        db_init(db_path).await.map_err(|e| {
                            crate::BuildonomyError::Custom(format!(
                                "Failed to initialise file-backed belief DB: {e}"
                            ))
                        })?
                    } else {
                        db_init_memory().await.map_err(|e| {
                            crate::BuildonomyError::Custom(format!(
                                "Failed to initialise in-memory belief DB: {e}"
                            ))
                        })?
                    };
                    BeliefAccumulator::new(DbConnection(db_pool), rx)
                };
                #[cfg(not(feature = "service"))]
                let accumulator = {
                    use crate::beliefbase::BeliefBase;
                    BeliefAccumulator::new(BeliefBase::empty(), rx)
                };

                // `query_handle()` is a cheap, clonable view backed by the same
                // `Arc<Mutex<AccInner>>` and `Arc<AccCache>`.  Pass this to `parse_all`
                // so parallel tasks can call `evaluate` without exclusive channel access.
                let global_bb = accumulator.query_handle();

                // Create compiler with event transmitter
                let mut compiler = if let Some(ref html_dir) = html_output {
                    std::fs::create_dir_all(html_dir)?;
                    DocumentCompiler::with_html_output(
                        &path,
                        Some(tx),
                        None,
                        write,
                        Some(html_dir.clone()),
                        None, // No live reload script for parse command
                        cdn,
                        base_url,
                        jobs,
                        git_tracking,
                    )?
                } else {
                    let mut c = DocumentCompiler::with_html_output(
                        &path,
                        Some(tx),
                        None,
                        write,
                        None,
                        None,
                        false,
                        None,
                        jobs,
                        git_tracking,
                    )?;
                    if let Some(j) = jobs {
                        c.set_jobs(j);
                    }
                    c
                };
                compiler.set_layout_config(layout_config);

                // Attach a progress bar unless --no-progress was passed or stderr is not a TTY.
                let show_progress = !no_progress && std::io::stderr().is_terminal();
                if show_progress {
                    compiler.with_progress(crate::codec::compiler::ProgressReporter::new());
                }

                // Parse all documents.  Events travel over `tx` into the accumulator.
                // `global_bb` is a `QueryHandle` that shares the accumulator's
                // `Arc<Mutex<AccInner>>`; each `BeliefSource` call on the handle locks
                // `AccInner` and consults `inner` through the shared cache.
                let parse_results = compiler.parse_all(global_bb, force).await?;

                // Get stats
                let stats = compiler.stats();

                // Close tx so the accumulator's channel is disconnected.
                // All epoch boundaries are signalled via BatchStart/BatchEnd on the
                // channel; closing tx is sufficient — no out-of-band drain needed.
                compiler.builder_mut().close_tx();

                // Extract the fully-populated backing store for post-parse operations.
                // With `service`: a DbConnection holding the committed belief graph.
                // Without `service`: a BeliefBase with all events applied in-memory.
                let final_bb = accumulator.into_inner().await.map_err(|e| {
                    crate::BuildonomyError::Custom(format!(
                        "BeliefAccumulator::into_inner failed: {}",
                        e
                    ))
                })?;

                // Optional const-namespace membership dump (NOET_DUMP_NAMESPACES=<path>).
                // Writes the full href/asset PathMap contents so the namespace's real
                // shape can be analysed offline — which URL prefixes dominate, and how
                // much of the membership is derived-BID stubs versus alias-owned content
                // nodes.  Diagnostic-only; absent the env var this is a no-op.
                if let Ok(dump_path) = std::env::var("NOET_DUMP_NAMESPACES") {
                    dump_const_namespaces(&final_bb, &dump_path).await;
                }

                // Finalize HTML generation with the synchronized backing store.
                // Pass a clone: finalize_html requires B: BeliefSource + Clone by value.
                let finalize_diagnostics = if html_output.is_some() {
                    compiler.finalize_html(final_bb.clone()).await?
                } else {
                    Vec::new()
                };

                // Collect and report diagnostics
                let colors = DiagColors::new(&color_choice);
                let mut warning_count = 0usize;
                let mut error_count = 0usize;

                // Report export-phase diagnostics (e.g. oversized networks) before
                // per-file diagnostics so authors see them prominently.
                for diagnostic in &finalize_diagnostics {
                    match diagnostic {
                        ParseDiagnostic::Warning { message: msg, .. } => {
                            let label = format!("{}warning{}", colors.warning, colors.reset);
                            eprintln!("{label}: {msg}");
                            warning_count += 1;
                        }
                        ParseDiagnostic::Info { message: msg, .. } if verbose => {
                            let label = format!("{}info{}", colors.info, colors.reset);
                            eprintln!("{label}: {msg}");
                        }
                        _ => {}
                    }
                }

                for result in &parse_results {
                    let path = result.path.display();
                    for diagnostic in &result.diagnostics {
                        match diagnostic {
                            ParseDiagnostic::Warning {
                                message: msg,
                                location,
                            } => {
                                let label = format!("{}warning{}", colors.warning, colors.reset);
                                if let Some((line, col)) = location {
                                    eprintln!("{path}:{line}:{col}: {label}: {msg}");
                                } else {
                                    eprintln!("{path}: {label}: {msg}");
                                }
                                warning_count += 1;
                            }
                            ParseDiagnostic::ReparseLimitExceeded => {
                                let label = format!("{}error{}", colors.error, colors.reset);
                                eprintln!("{path}: {label}: reparse limit exceeded");
                                error_count += 1;
                            }
                            ParseDiagnostic::ParseError {
                                message,
                                attempt_count,
                                location,
                            } => {
                                let label = format!("{}error{}", colors.error, colors.reset);
                                if let Some((line, col)) = location {
                                    eprintln!(
                                        "{path}:{line}:{col}: {label}: {} (after {} attempt{})",
                                        message,
                                        attempt_count,
                                        if *attempt_count == 1 { "" } else { "s" }
                                    );
                                } else {
                                    eprintln!(
                                        "{path}: {label}: {} (after {} attempt{})",
                                        message,
                                        attempt_count,
                                        if *attempt_count == 1 { "" } else { "s" }
                                    );
                                }
                                error_count += 1;
                            }
                            ParseDiagnostic::Info {
                                message: msg,
                                location,
                            } => {
                                if verbose {
                                    let label = format!("{}info{}", colors.info, colors.reset);
                                    if let Some((line, col)) = location {
                                        eprintln!("{path}:{line}:{col}: {label}: {msg}");
                                    } else {
                                        eprintln!("{path}: {label}: {msg}");
                                    }
                                }
                            }
                            // UnresolvedReference diagnostics are compiler-internal signals
                            // and must not survive past parse_all; nothing to display.
                            ParseDiagnostic::UnresolvedReference(_) => {}
                        }
                    }
                }

                if warning_count > 0 || error_count > 0 {
                    eprintln!(
                        "\n{} warning{}, {} error{}",
                        warning_count,
                        if warning_count == 1 { "" } else { "s" },
                        error_count,
                        if error_count == 1 { "" } else { "s" },
                    );
                }

                if verbose {
                    println!("\n=== Parse Results ===");
                    println!("Remainder  queue: {}", stats.remainder_queue_len);
                    println!("Processed: {}", stats.processed_count);
                    println!("Total parses: {}", stats.total_parses);

                    if write {
                        println!("\n=== Write Results ===");
                        println!("Files processed: {}", stats.processed_count);
                        println!("Note: Only modified files are written back");
                    }
                } else {
                    println!(
                        "Processed {} file{} ({} parse{})",
                        stats.processed_count,
                        if stats.processed_count == 1 { "" } else { "s" },
                        stats.total_parses,
                        if stats.total_parses == 1 { "" } else { "s" },
                    );
                }

                // HTML generation and export handled by finalize_html above

                if error_count > 0 {
                    std::process::exit(1);
                }

                Ok::<(), crate::BuildonomyError>(())
            })?;

            if verbose {
                println!("\nParsing completed successfully");
            }

            Ok(())
        }

        #[cfg(feature = "service")]
        Commands::Watch {
            path,
            verbose,
            config,
            write,
            html_output,
            cdn,
            base_url,
            serve,
            port,
            git_tracking,
        } => {
            // Read base_url from environment if not provided via CLI
            let base_url = base_url.or_else(|| std::env::var("NOET_BASE_URL").ok());
            // Resolve git_tracking: CLI flag > NOET_GIT_TRACKING env var > false.
            let git_tracking = git_tracking
                || std::env::var("NOET_GIT_TRACKING")
                    .ok()
                    .map(|v| v == "1" || v.eq_ignore_ascii_case("true"))
                    .unwrap_or(false);
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
                        cdn,
                        base_url,
                        git_tracking,
                    )?
                } else {
                    WatchService::new(root_dir.clone(), tx, write, git_tracking)?
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
                            let dev_server = crate::dev_server::DevServer::new(html_dir, port);

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

        #[cfg(feature = "mcp")]
        Commands::Mcp {
            output_dir,
            watch,
            html_output,
            git_tracking,
        } => {
            if let Some(ref watch_path) = watch {
                // ── Live mode ─────────────────────────────────────────────────
                // Spin up a WatchService, wait for the first compile pass to
                // complete, then hand its DbConnection to run_mcp_server_live.
                #[cfg(not(feature = "service"))]
                {
                    eprintln!("Error: --watch requires the 'service' feature.");
                    eprintln!("Rebuild with: cargo build --features mcp,service");
                    std::process::exit(1);
                }
                #[cfg(feature = "service")]
                {
                    use crate::watch::WatchService;
                    use std::sync::mpsc::channel;
                    use std::time::Duration;

                    let (tx, _rx) = channel();
                    let git_tracking = git_tracking
                        || std::env::var("NOET_GIT_TRACKING")
                            .ok()
                            .map(|v| v == "1" || v.eq_ignore_ascii_case("true"))
                            .unwrap_or(false);

                    let service = if let Some(ref html_dir) = html_output {
                        std::fs::create_dir_all(html_dir)?;
                        WatchService::with_html_output(
                            watch_path.clone(),
                            tx,
                            false, // read-only: MCP doesn't write back
                            Some(html_dir.clone()),
                            None,
                            false,
                            None,
                            git_tracking,
                        )?
                    } else {
                        WatchService::new(watch_path.clone(), tx, false, git_tracking)?
                    };

                    service.enable_network_syncer(watch_path)?;

                    // Wait for the first full compile pass before serving.
                    tracing::info!("MCP live mode: waiting for initial compile pass...");
                    service
                        .wait_for_idle(Duration::from_secs(120))
                        .map_err(|e| {
                            eprintln!("MCP live mode: timed out waiting for compile: {e}");
                            std::process::exit(1);
                        })
                        .ok();
                    tracing::info!("MCP live mode: compile pass complete, starting server");

                    crate::mcp::run_mcp_server_live(service.db_connection(), html_output, &service)
                        .unwrap_or_else(|e| {
                            eprintln!("MCP server error: {e}");
                            std::process::exit(1);
                        });
                }
            } else {
                // ── Static mode ───────────────────────────────────────────────
                crate::mcp::run_mcp_server(output_dir).unwrap_or_else(|e| {
                    eprintln!("MCP server error: {e}");
                    std::process::exit(1);
                });
            }
            Ok(())
        }

        #[cfg(feature = "distribute")]
        Commands::Distribute {
            site_path,
            target,
            port,
        } => {
            let target_dir = target.unwrap_or_else(|| {
                // Normalize through components to strip trailing separators:
                // "horizon/" → "horizon" → "horizon_dist" (sibling),
                // not "horizon/_dist" (child, which causes infinite recursion).
                let clean: PathBuf = site_path.components().collect();
                let mut t = clean.as_os_str().to_owned();
                t.push("_dist");
                PathBuf::from(t)
            });
            eprintln!(
                "Packaging {} → {}",
                site_path.display(),
                target_dir.display()
            );
            crate::distribute::distribute(&site_path, &target_dir, port)?;
            eprintln!("Distribution ready: {}", target_dir.display());
            Ok(())
        }
    }
}
