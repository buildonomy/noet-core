# noet-core

A Rust library for parsing interconnected documents into a queryable hypergraph with bidirectional synchronization.

## What is noet-core?

**noet-core** (from "noetic" - relating to knowledge and intellect) transforms document networks (Markdown, TOML, etc.) into a queryable hypergraph structure called a "BeliefSet". It maintains **bidirectional synchronization** between human-readable source files and a machine-queryable graph, automatically managing cross-document references and propagating changes.

### Key Features

- **Multi-pass compilation**: Diagnostic-driven resolution of forward references and circular dependencies
- **Stable identifiers**: Automatically injects unique BIDs (Belief IDs) into source documents for stable cross-document linking
- **Bidirectional sync**: Changes flow from documents to graph *and* from graph back to documents
- **Error tolerance**: Graceful handling of parse errors via diagnostic system - compilation never fails catastrophically
- **Multi-format support**: Extensible codec system (Markdown, TOML) with custom format support
- **Hypergraph relationships**: Rich semantic relationships with typed edges and custom payloads
- **Nested networks**: Hierarchical network dependencies similar to git submodules
- **Event streaming**: Incremental cache updates via event-driven architecture

## Quick Start

```rust
use noet_core::{codec::BeliefSetParser, beliefset::BeliefSet};

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    // Create parser (simple convenience constructor)
    let mut parser = BeliefSetParser::simple("./docs")?;
    
    // Stand-in for our global-cache (No effect when used in BeliefSetParser::simple() constructor but
    // used to access DB-backed version of our BeliefSet if available).
    let cache = BeliefSet::default()
    
    // Parse all documents (handles multi-pass resolution automatically)
    let results = parser.parse_all(BeliefSet::default()).await?;
    
    // Access the compiled graph
    let belief_set = parser.accumulator().set();
    
    // Query nodes
    for (bid, node) in belief_set.states() {
        println!("{}: {}", node.title, bid);
    }
    
    // Inspect diagnostics (unresolved refs, warnings, etc.)
    for result in results {
        for diagnostic in result.diagnostics {
            println!("{:?}", diagnostic);
        }
    }
    
    Ok(())
}
```

## How It Works

### Multi-Pass Compilation

noet-core implements a compiler-like system for document networks:

1. **First Pass**: Parse all files, collect unresolved references as diagnostics
2. **Resolution Passes**: Reparse files with resolved dependencies, inject BIDs, create relations
3. **Convergence**: Iterate until all resolvable references are linked
4. **Incremental Updates**: File changes trigger selective reparsing of affected documents

See the [Architecture Guide](docs/design/architecture.md) for details.

### The BID System

Every node gets a **BID** (Belief ID) - a UUID injected into the source document:

```toml
# Before first parse (user-authored)
id = "my_document"
title = "My Document"

# After compilation (BID injected by system)
bid = "01234567-89ab-cdef-0123-456789abcdef"
id = "my_document"
title = "My Document"
```

**Why BIDs matter**:
- Links survive file renames and moves
- Enables merging graphs without ID collisions
- Provides stable identity across distributed systems

### Diagnostic-Driven Resolution

Unresolved references are tracked as **diagnostics, not errors**:

```rust
pub enum ParseDiagnostic {
    UnresolvedReference(UnresolvedReference),  // Forward ref (will resolve later)
    SinkDependency { path, bid },               // Document references changed content
    Warning(String),
    Info(String),
}
```

The parser automatically tracks and resolves references across multiple passes.

## Use Cases

### Knowledge Management
- Build personal knowledge bases with automatic link maintenance
- Bidirectional linking between documents
- Auto-updating WikiLink titles when content changes
- Graph visualization of document networks

### Documentation Systems
- Maintain large, interconnected documentation
- Cross-document reference validation
- Multi-format support (Markdown, TOML, custom codecs)
- Incremental compilation for fast rebuilds

### Custom Applications
- Extend with custom schemas and relationship types
- Build domain-specific document processing pipelines
- Integrate with databases for persistent storage
- Create reactive UIs with event streaming

## Architecture

```
Source Files (*.md, *.toml)
    ↓
[Parse] → DocCodec implementations
    ↓
ProtoBeliefNode (IR)
    ↓
[Link] → BeliefSetAccumulator (multi-pass)
    ↓
BeliefSet (Compiled Graph)
    ↓
[Query/Traverse] → Application logic
```
See the [Architecture Guide](docs/architecture.md) for details.

### Core Components

- **`beliefset`**: Hypergraph data structures (BeliefSet, BidGraph)
- **`codec`**: Document parsing (BeliefSetParser, DocCodec trait)
- **`properties`**: Node/edge types, identifiers (BID), relationship semantics
- **`event`**: Event streaming for cache synchronization
- **`query`**: Query language for graph traversal
- **`paths`**: Relative path resolution across nested networks

## Comparison to Other Tools

| Feature | noet-core | Obsidian | Neo4j | rust-analyzer |
|---------|-----------|----------|-------|---------------|
| Bidirectional doc-graph sync | ✅ | Partial | ❌ | ❌ |
| BID injection into source | ✅ | ❌ | ❌ | ❌ |
| Multi-pass forward refs | ✅ | ❌ | ❌ | ✅ |
| Hypergraph structure | ✅ | ❌ | ✅ | ❌ |
| Multi-format parsing | ✅ | ✅ | Via plugins | ✅ |
| Nested networks | ✅ | ❌ | ❌ | Workspace |
| Error-tolerant parsing | ✅ | Partial | ❌ | ✅ |
| Schema extensibility | ✅ | Via plugins | ✅ | ❌ |

**Unique Combination**: noet-core brings together compiler techniques (multi-pass resolution, diagnostics), knowledge management (bidirectional linking), and hypergraph structures in a single library.

## Installation

Add to your `Cargo.toml`:

```toml
[dependencies]
noet-core = "0.1.0"
```

With optional features:

```toml
[dependencies]
noet-core = { version = "0.0.0", features = ["service"] }
```

### Features

- **default**: Core parsing and graph construction
- **service**: daemon service for file watching (`notify`), and managing a SQLite database integration (`sqlx`).
- **wasm**: WebAssembly support (`serde-wasm-bindgen`, `uuid/js`)

## Documentation

- **[Architecture Overview](docs/architecture.md)** - High-level concepts and design
- **[Design Specification](docs/design/beliefset_architecture.md)** - Detailed technical specification
- **[API Documentation](https://docs.rs/noet-core)** - Generated from source (run `cargo doc --open`)
- **[Examples](examples/)** - Working code examples

## Examples

### Basic Parsing

```rust
use noet_core::{beliefset::BeliefSet, codec::BeliefSetParser};

let mut parser = BeliefSetParser::simple("./docs")?;
// Stand-in for our global-cache (No effect when used in BeliefSetParser::simple() constructor but
// used to access DB-backed version of our BeliefSet if available).
let cache = BeliefSet::default()
let results = parser.parse_all(cache).await?;
let accumulated_set = parser.accumulator().stack_cache();
```

### With Diagnostics

```rust
for result in results {
    for diagnostic in result.diagnostics {
        match diagnostic {
            ParseDiagnostic::UnresolvedReference(unresolved) => {
                println!("Forward ref: {:?}", unresolved);
            }
            ParseDiagnostic::Warning(msg) => {
                println!("Warning: {}", msg);
            }
            _ => {}
        }
    }
}
```

See `examples/` directory for more complete examples.

## Development

```bash
# Build the library
cargo build --all-features

# Run tests
cargo test --all-features

# Generate documentation
cargo doc --no-deps --all-features --open

# Run examples
cargo run --example basic_usage --features service
```

## Status

⚠️ **Pre-1.0**: This library is under active development. The API may change before v1.0.0. Feedback and contributions are welcome!

## License

Licensed under either of:

- Apache License, Version 2.0 ([LICENSE-APACHE](LICENSE-APACHE) or http://www.apache.org/licenses/LICENSE-2.0)
- MIT license ([LICENSE-MIT](LICENSE-MIT) or http://opensource.org/licenses/MIT)

at your option.

### Contribution

Unless you explicitly state otherwise, any contribution intentionally submitted for inclusion in the work by you, as defined in the Apache-2.0 license, shall be dual licensed as above, without any additional terms or conditions.

## Contributing

See [CONTRIBUTING.md](CONTRIBUTING.md) for guidelines.

## Acknowledgments

noet-core draws inspiration from:
- Knowledge management tools: Obsidian, Roam Research, Logseq
- Language servers: rust-analyzer, tree-sitter
- Graph databases: Neo4j
- Hypergraph systems: HIF, Hypergraphx

The name "noet" comes from "noetic", relating to knowledge and the intellect.
