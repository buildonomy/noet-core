# Documentation Strategy

**Purpose**: Explain how documentation is organized in noet-core to avoid duplication and establish clear sources of truth.

**Last Updated**: 2025-01-17

## Overview

noet-core follows Rust ecosystem best practices for documentation organization, similar to successful libraries like `tokio`, `serde`, and `diesel`. The goal is to have **one source of truth** for each type of information while maintaining discoverability.

## Documentation Hierarchy

### 1. `src/lib.rs` Rustdoc: "Getting Started Guide"

**Purpose**: Help developers understand what the library does and start using it quickly

**Target Audience**: New users, quick reference

**Content**:
- Brief overview (2-3 paragraphs)
- Key features (bullet list)
- Architecture component list (one-line descriptions)
- Quick start examples (working doctests)
- Core concepts (brief explanations)
- Comparison to other tools (high-level only)
- Links to detailed documentation

**Length**: ~100-150 lines

**Source of Truth For**: "What is this library and how do I use it?"

**Does NOT contain**:
- Detailed architectural specifications
- Algorithm explanations
- Design rationale
- Implementation details
- Future work discussion

### 2. `docs/architecture.md`: "High-Level Architecture"

**Purpose**: Explain core concepts and architecture for developers getting started with the library

**Target Audience**: Developers who want to understand how the library works

**Content**:
- Core concepts (BID, BeliefBase, multi-pass compilation)
- Architecture overview (components and their roles)
- Data flow diagrams
- Relationship to prior art (detailed comparisons)
- Unique features
- Getting started examples (with explanations)
- Use cases

**Length**: ~250-300 lines

**Source of Truth For**: "How does the library work at a conceptual level?"

**Links to**: `docs/design/beliefbase_architecture.md` for technical details

### 3. `docs/design/beliefbase_architecture.md`: "Technical Specification"

**Purpose**: Complete technical specification for understanding internals, contributing, or making architectural decisions

**Target Audience**: Contributors, maintainers, advanced users

**Content**:
- Detailed purpose and compilation model
- Identity management specifications (BID, Bref, NodeKey)
- Graph structure and invariants
- Architecture diagrams with data structures
- Algorithm specifications (multi-pass resolution, stack-based parsing)
- Code structure documentation
- Integration points
- Examples with detailed explanations
- Architectural concerns and future enhancements

**Length**: ~700-800 lines

**Source of Truth For**: "How is this implemented and why?"

**References**: Code locations (e.g., `beliefbase.rs:660-2420`)

### 4. Module-Level Rustdoc: "API Guide"

**Purpose**: Explain how to use specific modules and types

**Target Audience**: Developers actively using the API

**Content** (each module):
- Module purpose
- Key types and traits
- Usage examples
- Common patterns
- Links to related modules

**Examples**:
- `src/beliefbase.rs`: BeliefBase data structure and operations
- `src/codec/mod.rs`: Parsing and codec system
- `src/properties.rs`: Node and edge types

**Source of Truth For**: "How do I use this specific API?"

### 5. `README.md`: "Project Overview"

**Purpose**: Quick introduction for GitHub/GitLab visitors

**Target Audience**: Everyone (first point of contact)

**Content**:
- What is noet-core?
- Key features
- Quick start example
- Installation instructions
- Links to documentation
- Comparison table
- Development instructions
- License and contributing

**Source of Truth For**: "Should I use this library?"

## Information Flow

```
README.md
    ↓ (Quick start)
src/lib.rs (rustdoc)
    ↓ (Learn concepts)
docs/architecture.md
    ↓ (Deep dive)
docs/design/beliefbase_architecture.md
    ↓ (Use API)
Module-level rustdoc
```

## Avoiding Duplication: The DRY Principle

### Rule 1: Link, Don't Duplicate

**Bad**:
```rust
// lib.rs
//! Multi-pass compilation works by:
//! 1. First pass: Parse all files
//! 2. Second pass: Resolve references
//! 3. Third pass: Inject BIDs
//! [... 50 lines of detailed algorithm ...]
```

**Good**:
```rust
// lib.rs
//! noet-core implements multi-pass compilation to handle forward references.
//! See `docs/design/beliefbase_architecture.md` for algorithm specification.
```

### Rule 2: Brief in Rustdoc, Detailed in Design Docs

**lib.rs example**:
```rust
//! ### Multi-Pass Compilation
//!
//! 1. **First Pass**: Parse all files, collect unresolved references
//! 2. **Resolution Passes**: Reparse with resolved dependencies
//! 3. **Convergence**: Iterate until complete
//!
//! See `docs/design/beliefbase_architecture.md` for details.
```

**design doc example**:
```markdown
### 3.0. Multi-Pass Compilation Algorithm

**First Pass - Virgin Repository**:
- Parses all files without prior context
- Target not yet parsed → `cache_fetch()` returns `GetOrCreateResult::Unresolved(...)`
- Collect `UnresolvedReference` diagnostic (no relation created yet)
- Compiler tracks unresolved refs for later resolution checking
- Continue parsing all files

[... detailed algorithm specification ...]
```

### Rule 3: Examples Can Duplicate (They're Specifications)

It's OK to have similar examples in multiple places if they serve different purposes:

- **lib.rs**: Minimal "hello world" example
- **architecture.md**: Slightly more complete example with explanation
- **module rustdoc**: Focused example for that specific module
- **examples/**: Full working programs

Each example should be maintained independently.

### Rule 4: Cross-Reference Aggressively

Use markdown links and rustdoc links to connect related content:

```rust
//! See [`beliefbase::BeliefBase`] for graph operations.
//! See `docs/architecture.md` for conceptual overview.
//! See `docs/design/beliefbase_architecture.md` for implementation details.
```

## When Content Overlaps: Decision Matrix

| Content Type | lib.rs | architecture.md | design spec | Module doc |
|--------------|--------|-----------------|-------------|------------|
| What is noet-core? | Brief | Detailed | No | No |
| How to install | No | No | No | No (in README) |
| Quick start example | Yes | Yes (enhanced) | No | No |
| Core concepts | Brief | Detailed | Very detailed | Focused |
| Architecture diagram | No | Yes (simple) | Yes (detailed) | No |
| Algorithm spec | No | No | Yes | No |
| API usage | Example | No | No | Yes |
| Design rationale | No | No | Yes | No |
| Future work | No | Brief | Detailed | No |
| Comparison to tools | Brief | Detailed | No | No |

## Maintenance Workflow

### When to Update Each Document

**lib.rs**:
- API changes that affect basic usage
- New core features
- Changes to quick start workflow

**architecture.md**:
- Major architectural changes
- New core concepts
- Updated comparisons to other tools

**design spec**:
- Algorithm changes
- Data structure modifications
- New architectural patterns
- Design decisions

**Module rustdoc**:
- API changes in that module
- New usage patterns
- Deprecations

### Update Checklist

When making significant changes:

1. **Code change** → Update module rustdoc
2. **Algorithm change** → Update design spec
3. **New concept** → Update architecture.md (and design spec if detailed)
4. **Breaking change** → Update lib.rs quick start if affected

## Benefits of This Approach

1. **Single Source of Truth**: Each type of information has one authoritative location
2. **Discoverability**: Clear progression from simple (lib.rs) to detailed (design spec)
3. **Maintainability**: Changes propagate naturally through links
4. **Rust Ecosystem Fit**: Follows patterns from successful Rust libraries
5. **Multiple Audiences**: Different docs for different needs

## Examples from Other Rust Libraries

### tokio

- **lib.rs**: Brief overview, features list, quick example, links to guides
- **Website guides**: Conceptual tutorials (like our `architecture.md`)
- **Module docs**: Detailed API usage
- No separate design spec (smaller scope)

### serde

- **lib.rs**: Very brief, links to guide
- **Website guide**: Comprehensive tutorial (like our `architecture.md`)
- **Module docs**: Derive macro details, API reference
- Design decisions in separate "data model" doc

### diesel

- **lib.rs**: Overview, quick start, links to guides
- **Website guides**: Getting started, advanced features
- **Module docs**: Detailed trait and macro docs
- Architecture in separate RFC-style documents

## Our Pattern

noet-core follows this pattern:

```
lib.rs (rustdoc)          ← tokio/serde style: brief, links to guides
    ↓
architecture.md            ← Website guide equivalent: conceptual
    ↓
design/beliefbase_architecture.md  ← RFC-style: technical spec
    ↓
Module rustdoc            ← Standard Rust: API reference
```

## Decision: Source of Truth for Each Topic

| Topic | Source of Truth | Also Mentioned In |
|-------|----------------|-------------------|
| Multi-pass compilation concept | lib.rs (brief) | architecture.md (detailed), design spec (algorithm) |
| Multi-pass algorithm | design spec | - |
| BID system concept | lib.rs (brief) | architecture.md (example), design spec (full spec) |
| BID implementation | design spec | - |
| BeliefBase API | BeliefBase module doc | lib.rs (mention), architecture.md (concept) |
| Comparison to other tools | architecture.md | lib.rs (brief), README (table) |
| Getting started | lib.rs | README (simpler), architecture.md (enhanced) |
| Architecture components | lib.rs (list) | architecture.md (explanation), design spec (detailed) |

## Conclusion

By following this strategy:

1. **Developers find what they need quickly** (clear progression from simple to detailed)
2. **Maintenance is straightforward** (one source of truth per topic)
3. **Documentation stays in sync** (links instead of duplication)
4. **Follows Rust best practices** (pattern-matches successful libraries)

When in doubt: **Brief in rustdoc, detailed in design docs, link aggressively**.
