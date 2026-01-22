//! Basic usage example for noet
//!
//! This example demonstrates:
//! - Creating a BeliefSet
//! - Parsing documents from a directory
//! - Querying the resulting graph
//!
//! Run with: cargo run --example basic_usage

use noet_core::{
    beliefset::BeliefSet,
    codec::{BeliefSetParser, ParseDiagnostic},
    BuildonomyError,
};
use petgraph::visit::EdgeRef;
use std::path::Path;
use tempfile::TempDir;

#[tokio::main(flavor = "current_thread")]
async fn main() -> Result<(), BuildonomyError> {
    // Set up logging to see what's happening
    tracing_subscriber::fmt()
        .with_max_level(tracing::Level::INFO)
        .init();

    println!("=== noet Basic Usage Example ===\n");

    // Create a temporary directory for our example documents
    let temp_dir = TempDir::new().expect("Failed to create temp dir");
    let docs_path = temp_dir.path().to_path_buf();

    // Create some example markdown documents
    create_example_documents(&docs_path)?;

    // 2. Set up parser configuration
    println!("2. Configuring parser...");
    // 3. Create parser and parse all documents
    println!("3. Parsing documents from {docs_path:?}...");
    let mut parser = BeliefSetParser::simple(docs_path)?;

    // Initial parse - this will do multiple passes to resolve forward references
    let mut parse_results = parser.parse_all(BeliefSet::default()).await?;
    let diagnostics: Vec<ParseDiagnostic> = parse_results
        .drain(..)
        .flat_map(|pr| pr.diagnostics)
        .collect();

    println!("   ✓ Parsed {} nodes\n", parser.cache().states().len());

    // 4. Query the graph
    println!("4. Querying the graph:");

    // Get all document nodes
    let doc_nodes: Vec<_> = parser
        .cache()
        .states()
        .values()
        .filter(|node| node.kind.is_document())
        .collect();

    println!("   Found {} documents:", doc_nodes.len());
    for node in &doc_nodes {
        println!("   - {}", node.display_title());
    }
    println!();

    // 5. Explore relationships
    println!("5. Exploring relationships:");
    for node in &doc_nodes {
        let edge_idx = parser.cache().bid_to_index(&node.bid);
        if let Some(idx) = edge_idx {
            println!("   Document '{}' links to:", node.display_title());
            for edge in parser.cache().relations().as_graph().edges(idx) {
                let source = parser.cache().relations().as_graph()[edge.source()];
                let sink = parser.cache().relations().as_graph()[edge.target()];
                let (other, direction) = if source == node.bid {
                    (sink, "↦")
                } else {
                    (source, "↤")
                };
                if let Some(target) = parser.cache().states().get(&other) {
                    println!(
                        "     {} {} ({})",
                        direction,
                        target.display_title(),
                        target.kind
                    );
                }
            }
        }
    }
    println!();

    // 6. Demonstrate BID stability
    println!("6. BID system:");
    println!("   Each node has a unique BID that remains stable even if");
    println!("   the document is renamed or moved. Example:");
    if let Some(node) = doc_nodes.first() {
        println!("   {}", node.to_string().replace("\n", "\n   "));
    }
    println!();

    // 7. Show diagnostics
    println!("7. Parser diagnostics:");
    if diagnostics.is_empty() {
        println!("   ✓ No issues found!");
    } else {
        println!("   Found {} diagnostic messages:", diagnostics.len());
        let mut unresolved = Vec::default();
        let mut parse_error = Vec::default();
        let mut warning = Vec::default();
        let mut info = Vec::default();
        for (idx, diag) in diagnostics.iter().enumerate() {
            match diag {
                ParseDiagnostic::UnresolvedReference(..) => {
                    unresolved.push(idx);
                }
                ParseDiagnostic::ParseError { .. } => {
                    parse_error.push(idx);
                }
                ParseDiagnostic::Warning(..) => {
                    warning.push(idx);
                }
                ParseDiagnostic::Info(..) => {
                    info.push(idx);
                }
            }
        }
        println!(
            "    {} parse errors\n
                 {} unresolved references\n
                 {} warnings\n
                 {} info messages
            ",
            parse_error.len(),
            unresolved.len(),
            warning.len(),
            info.len()
        );
    }
    println!();

    println!("=== Example Complete ===");
    println!("\nNext steps:");
    println!("  - Check out the docs: https://docs.rs/noet-core");
    println!("  - See more examples: cargo run --example file_watching");
    println!("  - Read the guide: https://gitlab.com/buildonomy/noet-core\n");

    Ok(())
}

/// Create some example markdown documents in the temporary directory
fn create_example_documents(base_path: &Path) -> std::io::Result<()> {
    use std::fs;

    // Create index.md
    fs::write(
        base_path.join("index.md"),
        r#"# Welcome to My Knowledge Base

This is the main index document. It links to:

- [[getting-started]] - How to get started
- [[concepts]] - Core concepts

## About

This knowledge base demonstrates noet's ability to:
- Parse markdown documents
- Resolve cross-document references
- Build a queryable hypergraph
- Inject stable BIDs for permanent references
"#,
    )?;

    // Create getting-started.md
    fs::write(
        base_path.join("getting-started.md"),
        r#"# Getting Started

Welcome! This guide will help you understand the basics.

## Prerequisites

You should read the [[concepts]] first.

## Quick Start

1. Install the library
2. Create a BeliefSet
3. Parse your documents
4. Query the graph

## Next Steps

Return to the [[index]] for more topics.
"#,
    )?;

    // Create concepts.md
    fs::write(
        base_path.join("concepts.md"),
        r#"# Core Concepts

## BeliefSet

A BeliefSet is a hypergraph that stores:
- Nodes (documents, sections, metadata). Each node specifies its schema
- Edges (relationships between nodes). Each edge has a type (currently Section, Epistemic, Pragmatic)
- Nodes and Edges can each contain a schema-specific Payload (key-value data)

## BIDs

BID stands for Belief ID. Each node gets a unique identifier that:
- Remains stable across renames
- Gets injected into source documents
- Enables permanent cross-document links

## Multi-pass Compilation

The parser makes multiple passes to resolve forward references,
similar to a compiler handling forward declarations.

See [[getting-started]] to begin using these concepts.
"#,
    )?;

    Ok(())
}
