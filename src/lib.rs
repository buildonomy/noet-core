//! # noet-core
//!
//! A Rust library for parsing interconnected documents into a queryable hypergraph with bidirectional synchronization.
//!
//! The name "noet" comes from "noetic" - relating to knowledge and the intellect.
//!
//! ## Overview
//!
//! noet-core transforms document networks (Markdown, TOML, etc.) into a queryable hypergraph structure
//! called a "BeliefSet". It maintains **bidirectional synchronization** between human-readable source
//! files and a machine-queryable graph, automatically managing cross-document references and propagating
//! changes.
//!
//! ### Key Features
//!
//! - **Multi-pass compilation**: Diagnostic-driven resolution of forward references and circular dependencies
//! - **Stable identifiers**: Automatically injects unique BIDs (Belief IDs) into source documents
//! - **Bidirectional sync**: Changes flow from documents to graph *and* from graph back to documents
//! - **Multi-format support**: Extensible codec system (Markdown, TOML, custom formats)
//! - **Error tolerance**: Graceful handling of parse errors via diagnostic system
//! - **Hypergraph relationships**: Rich semantic relationships with typed edges and custom payloads
//! - **Nested networks**: Hierarchical network dependencies similar to git submodules
//! - **Event streaming**: Incremental cache updates via event-driven architecture
//!
//! ## Architecture
//!
//! The library is organized around several key components:
//!
//! - **[`beliefset`]**: Core hypergraph data structures (`BeliefSet`, `BidGraph`)
//! - **[`codec`]**: Document parsing (`BeliefSetParser`, `BeliefSetAccumulator`, `DocCodec` trait)
//! - **[`properties`]**: Node/edge types, identifiers (`Bid`), relationship semantics
//! - **[`event`]**: Event streaming for cache synchronization
//! - **[`query`]**: Query language for graph traversal and filtering
//! - **[`paths`]**: Relative path resolution across nested networks
//!
//! For detailed architecture documentation, see:
//! - High-level overview: `docs/architecture.md`
//! - Technical specification: `docs/design/beliefset_architecture.md`
//!
//! ## Quick Start
//!
//! ### Basic Parsing
//!
//! Parse a directory of documents into a BeliefSet:
//!
//! ```rust,no_run
//! use noet_core::{beliefset::BeliefSet, codec::BeliefSetParser};
//!
//! #[tokio::main(flavor = "current_thread")]
//! async fn main() -> Result<(), Box<dyn std::error::Error>> {
//!     // Create parser (simple convenience constructor)
//!     let mut parser = BeliefSetParser::simple("./docs")?;
//!     // Stand-in for our global-cache (No effect when used in BeliefSetParser::simple() constructor but
//!     // used to access DB-backed version of our BeliefSet if available).
//!     let cache = BeliefSet::default();
//!
//!     // Parse all documents (handles multi-pass resolution automatically)
//!     let results = parser.parse_all(cache).await?;
//!
//!     // Access the accumulated graph
//!     let belief_set = parser.accumulator().stack_cache();
//!
//!     // Query nodes
//!     for (bid, node) in belief_set.states() {
//!         println!("{}: {}", node.title, bid);
//!     }
//!
//!     Ok(())
//! }
//! ```
//!
//! ### Working with Diagnostics
//!
//! The parser tracks unresolved references and errors as diagnostics:
//!
//! ```rust,no_run
//! # use noet_core::{beliefset::BeliefSet, codec::{BeliefSetParser, ParseDiagnostic}};
//! # #[tokio::main(flavor = "current_thread")]
//! # async fn main() -> Result<(), Box<dyn std::error::Error>> {
//! # let mut parser = BeliefSetParser::simple("./docs")?;
//! # let cache = BeliefSet::default();
//! # let results = parser.parse_all(cache).await?;
//! for result in results {
//!     for diagnostic in result.diagnostics {
//!         match diagnostic {
//!             ParseDiagnostic::UnresolvedReference(unresolved) => {
//!                 println!("Forward ref: {:?}", unresolved);
//!             }
//!             ParseDiagnostic::Warning(msg) => {
//!                 println!("Warning: {}", msg);
//!             }
//!             _ => {}
//!         }
//!     }
//! }
//! # Ok(())
//! # }
//! ```
//!
//! ### File Watching (requires `service` feature)
//!
//! ```rust,no_run
//! # #[cfg(feature = "service")]
//! # async fn example() -> Result<(), Box<dyn std::error::Error>> {
//! use noet_core::codec::BeliefSetParser;
//! use noet_core::beliefset::BeliefSet;
//! use notify::{Watcher, RecursiveMode};
//! # use std::path::PathBuf;
//! # use tokio::sync::mpsc;
//! # let (tx, _rx) = mpsc::unbounded_channel();
//! # let cache = BeliefSet::default();
//! # let mut watcher = notify::recommended_watcher(|_| {}).unwrap();
//! # let modified_path = PathBuf::from("./docs/example.md");
//!
//! let mut parser = BeliefSetParser::new("./docs", Some(tx), None, true)?;
//!
//! // Initial parse
//! parser.parse_all(cache.clone()).await?;
//!
//! // Watch for file changes
//! watcher.watch("./docs", RecursiveMode::Recursive)?;
//!
//! // On file modification
//! parser.on_file_modified(modified_path);
//! if let Some(result) = parser.parse_next(cache.clone()).await? {
//!     // Handle reparse result
//! }
//! # Ok(())
//! # }
//! ```
//!
//! ## Core Concepts
//!
//! ### Multi-Pass Compilation
//!
//! noet-core implements a compiler-like system that handles forward references through multiple passes:
//!
//! 1. **First Pass**: Parse all files, collect unresolved references as diagnostics
//! 2. **Resolution Passes**: Reparse files with resolved dependencies, inject BIDs
//! 3. **Convergence**: Iterate until all resolvable references are linked
//!
//! See `docs/design/beliefset_architecture.md` for detailed algorithm specification.
//!
//! ### BID System
//!
//! Every node gets a **BID** (Belief ID) - a UUID injected into the source document:
//!
//! ```toml
//! # Before first parse
//! id = "my_document"
//! title = "My Document"
//!
//! # After compilation (BID injected)
//! bid = "01234567-89ab-cdef-0123-456789abcdef"
//! id = "my_document"
//! title = "My Document"
//! ```
//!
//! BIDs provide stable references that survive file renames and enable graph merging.
//!
//! ### Hypergraph Structure
//!
//! The BeliefSet is a typed, weighted, directed hypergraph where:
//! - **Nodes** are `BeliefNode` instances (documents, sections, custom entities)
//! - **Edges** are typed relationships (`WeightKind`: Subsection, Epistemic, Pragmatic)
//! - Each edge can carry custom metadata in its `payload`
//!
//! ### Diagnostic-Driven Resolution
//!
//! Unresolved references are tracked as diagnostics, not errors:
//!
//! ```rust,no_run
//! # use noet_core::codec::{ParseDiagnostic, UnresolvedReference};
//! # fn example() {
//! # let _diagnostic =
//! ParseDiagnostic::UnresolvedReference(UnresolvedReference::default())  // Forward ref (will resolve later)
//! # ;
//! # let _diagnostic =
//! ParseDiagnostic::Warning(String::from("example"))
//! # ;
//! # let _diagnostic =
//! ParseDiagnostic::Info(String::from("example"))
//! # ;
//! # }
//! ```
//!
//! The parser automatically tracks and resolves these across multiple passes.
//!
//! ## Comparison to Other Tools
//!
//! noet-core combines features from several domains:
//!
//! - **Knowledge management** (Obsidian, Roam): Bidirectional linking + automatic BID injection
//! - **Hypergraph libraries** (HGX, HIF): Rich graph structure + document management focus
//! - **Knowledge graphs** (Neo4j, Docs2KG): Graph construction + bidirectional doc-graph sync
//! - **Language servers** (rust-analyzer, tree-sitter): Error-tolerant parsing + multi-pass compilation
//!
//! **Unique combination**: Compiler techniques + knowledge management + hypergraph structures in a single library.
//!
//! See `docs/architecture.md` for detailed comparisons.
//!
//! ## Features
//!
//! - **default**: Core parsing and graph construction
//! - **service**: File watching (`notify`), database integration (`sqlx`)
//! - **wasm**: WebAssembly support
//!
//! ## Documentation
//!
//! - **Getting started**: `docs/architecture.md` (high-level concepts)
//! - **Technical spec**: `docs/design/beliefset_architecture.md` (detailed architecture)
//! - **API reference**: Module-level docs (run `cargo doc --open`)
//! - **Examples**: See `examples/` directory
//!
//! ## Module Guide
//!
//! Start with [`codec::BeliefSetParser`] for parsing documents, then explore [`beliefset::BeliefSet`]
//! for graph operations. See [`properties`] for understanding node and edge types.

pub mod beliefset;
pub mod codec;
#[cfg(feature = "service")]
pub mod commands;
#[cfg(feature = "service")]
pub mod compiler;
#[cfg(feature = "service")]
pub mod config;
#[cfg(feature = "service")]
pub mod db;
pub mod error;
pub mod event;
pub mod nodekey;
pub mod paths;
pub mod properties;
pub mod query;
#[cfg(test)]
mod tests;

pub use error::*;

uniffi::setup_scaffolding!();
