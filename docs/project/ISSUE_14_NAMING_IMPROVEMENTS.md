# Issue 14: Pedagogical Naming Improvements

**Priority**: MEDIUM - Improves clarity and maintainability  
**Estimated Effort**: 2-3 days  
**Dependencies**: None (can proceed anytime, but better before v0.1.0)  
**Context**: Improve naming to match compiler architecture analogy and reduce confusion

## Summary

Several type and field names in noet-core are pedagogically confusing and don't clearly convey their purpose or role in the compilation pipeline. This issue proposes renaming key types and fields to better match compiler architecture conventions and improve code readability.

**Key Problems**:
1. `BeliefSetParser` doesn't parse - it orchestrates (like a build system)
2. `BeliefSetAccumulator` does both parsing AND linking - not just accumulation
3. Field names are backwards: `.set` is local, `.stack_cache` is global
4. `BeliefSet` vs `Beliefs` distinction is unclear
5. Names don't align with the compiler analogy we document

## Goals

1. Rename types to match their actual responsibilities
2. Rename fields to be self-documenting
3. Align naming with compiler architecture terminology
4. Maintain consistency across the codebase
5. Update all documentation to reflect new names
6. Provide migration guide for users

## Current Naming Issues

### 1. BeliefSetParser (codec/parser.rs)

**Problem**: Name suggests it parses, but it actually orchestrates multi-pass compilation.

**What it does**:
- Manages work queue
- Coordinates which files get parsed when
- Drives multi-pass compilation to convergence
- Handles file watching integration

**Compiler analogy**: Build system, compilation driver (like `rustc` coordinator, `make`)

### 2. BeliefSetAccumulator (codec/mod.rs)

**Problem**: Name suggests it only accumulates, but it both parses files AND links references.

**What it does**:
- Parses files via DocCodec
- Maintains document stack for structure
- Resolves references (linking)
- Creates relations between nodes
- Publishes events

**Compiler analogy**: Semantic analyzer + linker

### 3. BeliefSetAccumulator.set

**Problem**: Misleading name - this is the LOCAL/CURRENT document's BeliefSet, not the accumulated result.

**What it is**: The BeliefSet for the currently-being-parsed document (document-scoped)

**Expected meaning**: The accumulated result (but it's not!)

### 4. BeliefSetAccumulator.stack_cache

**Problem**: Name doesn't convey that this is the SESSION-accumulated cache, not the global persistent cache.

**What it is**: The accumulated BeliefSet across documents parsed in this SESSION (in-memory, not persisted)

**Expected meaning**: Some temporary cache for the stack (but it's not!)

**Note**: The actual GLOBAL cache is the DB-backed `global_cache: B` parameter passed to the accumulator.

### 5. BeliefSet vs Beliefs

**Problem**: Distinction is unclear from names alone.

**What they are**:
- `BeliefSet`: Full-featured, indexed, queryable graph structure
- `Beliefs`: Lightweight transport structure (just states + relations)

## Three-Tier Caching Architecture

Understanding the three levels of scope is critical to understanding the confusing field names:

```
┌─────────────────────────────────────────────────────────────┐
│  Document-Scoped (.set)                                     │
│  - Currently-being-parsed document only                     │
│  - Cleared/reset between documents                          │
│  - BeliefSet for single file                                │
└─────────────────────────────────────────────────────────────┘
                         ↓ accumulates into
┌─────────────────────────────────────────────────────────────┐
│  Session-Scoped (.stack_cache)                              │
│  - Accumulated across all documents in this parse session   │
│  - In-memory only, lost on process exit                     │
│  - BeliefSet for all parsed files                           │
└─────────────────────────────────────────────────────────────┘
                         ↓ syncs to/from
┌─────────────────────────────────────────────────────────────┐
│  Global/Persistent (global_cache: B parameter)              │
│  - DB-backed, persists across sessions                      │
│  - Authoritative source for BID identity                    │
│  - Shared across all parsing sessions                       │
└─────────────────────────────────────────────────────────────┘
```

**The Problem**: Field names don't clearly indicate which scope they represent.

## Proposed Naming Scheme

**Three Levels of Scope**:
1. **Document-local**: Current document being parsed (`.set` → `.document_graph`)
2. **Session-local**: Accumulated in-memory during parsing session (`.stack_cache` → `.session_graph`)
3. **Global/persistent**: DB-backed cache passed as parameter (`global_cache: B` - already well-named)

### Option A: Traditional Compiler Terms

**Pros**: Clear analogy to compiler architecture  
**Cons**: Moves away from "Belief" terminology

| Current | Proposed | Rationale |
|---------|----------|-----------|
| `BeliefSetParser` | `DocumentCompiler` | Orchestrates compilation of document network |
| `BeliefSetAccumulator` | `GraphLinker` | Links documents into graph |
| `BeliefSet` | `IndexedGraph` | Indexed, queryable graph structure |
| `Beliefs` | `GraphData` | Raw graph data (states + relations) |
| `.set` | `.current_document` | Document-scoped (one file) |
| `.stack_cache` | `.session_cache` | Session-scoped (in-memory accumulation) |

### Option B: Keep "Belief" Terminology

**Pros**: Maintains domain terminology consistency  
**Cons**: Less obvious compiler analogy

| Current | Proposed | Rationale |
|---------|----------|-----------|
| `BeliefSetParser` | `BeliefCompiler` | Compiles document network into BeliefSet |
| `BeliefSetAccumulator` | `BeliefLinker` | Links beliefs across documents |
| `BeliefSet` | `IndexedBeliefSet` | Clarifies this is the indexed version |
| `Beliefs` | `RawBeliefs` | Clarifies this is the raw data |
| `.set` | `.document_set` | Document-scoped |
| `.stack_cache` | `.session_set` | Session-scoped (in-memory) |

### Option C: Hybrid Approach (RECOMMENDED)

**Pros**: Clear roles while keeping some domain terminology  
**Cons**: Mix of styles

| Current | Proposed | Rationale |
|---------|----------|-----------|
| `BeliefSetParser` | `DocumentCompiler` | Clear orchestration role |
| `BeliefSetAccumulator` | `GraphBuilder` | Builds graph from documents |
| `BeliefSet` | `BeliefSet` | Keep (well-known) |
| `Beliefs` | `BeliefGraph` | Clarifies it's graph data |
| `.set` | `.document_graph` | Document-scoped graph |
| `.stack_cache` | `.session_graph` | Session-scoped accumulation |

### Option D: Most Intuitive (ALTERNATIVE)

| Current | Proposed | Rationale |
|---------|----------|-----------|
| `BeliefSetParser` | `CompilationOrchestrator` | Most explicit |
| `BeliefSetAccumulator` | `DocumentLinker` | Clear linking role |
| `BeliefSet` | `QueryableGraph` | Emphasizes usage |
| `Beliefs` | `GraphSnapshot` | Lightweight snapshot |
| `.set` | `.current_doc` | Short, document-scoped |
| `.stack_cache` | `.session_cache` | Short, session-scoped |

## Implementation Steps

### Phase 1: Type Aliases (0.5 days)

Add type aliases to ease migration:

```rust
// In codec/parser.rs
#[deprecated(note = "Use DocumentCompiler instead")]
pub type BeliefSetParser = DocumentCompiler;

// In codec/mod.rs
#[deprecated(note = "Use GraphBuilder instead")]
pub type BeliefSetAccumulator = GraphBuilder;
```

### Phase 2: Rename Types (1 day)

1. **Rename struct definitions**:
   - [ ] `BeliefSetParser` → chosen name
   - [ ] `BeliefSetAccumulator` → chosen name
   - [ ] `Beliefs` → chosen name (if changing)

2. **Update all usages**:
   - [ ] Update imports across codebase
   - [ ] Update method names that reference old types
   - [ ] Update error messages

3. **Update tests**:
   - [ ] Rename test helper types
   - [ ] Update test documentation

### Phase 3: Rename Fields (0.5 days)

1. **In BeliefSetAccumulator/GraphBuilder**:
   - [ ] `.set` → `.document_graph` (or chosen name) - document-scoped
   - [ ] `.stack_cache` → `.session_graph` or `.session_cache` (or chosen name) - session-scoped

2. **Update all field accesses**:
   - [ ] Search for `.set` usages
   - [ ] Search for `.stack_cache` usages
   - [ ] Update with new names

**Note**: The `global_cache: B` parameter is already well-named and represents the true global/persistent cache.

### Phase 4: Documentation (1 day)

1. **Update design docs**:
   - [ ] `docs/design/beliefset_architecture.md`
   - [ ] `docs/architecture.md`
   - [ ] Update compiler analogy sections

2. **Update rustdoc**:
   - [ ] `src/lib.rs`
   - [ ] `src/codec/parser.rs`
   - [ ] `src/codec/mod.rs`
   - [ ] `src/beliefset.rs`

3. **Update examples**:
   - [ ] `examples/basic_usage.rs`
   - [ ] Any other examples

4. **Update README**:
   - [ ] Core README
   - [ ] Update code examples

### Phase 5: Migration Guide (0.5 days)

Create `docs/MIGRATION_v0.X.md`:

```markdown
# Migration Guide: Naming Changes

## Type Renames

- `BeliefSetParser` → `DocumentCompiler`
- `BeliefSetAccumulator` → `GraphBuilder`
- `Beliefs` → `BeliefGraph`

## Field Renames

- `BeliefSetAccumulator.set` → `GraphBuilder.document_graph` (document-scoped)
- `BeliefSetAccumulator.stack_cache` → `GraphBuilder.session_graph` (session-scoped, in-memory)

Note: The actual global cache is the DB-backed parameter, not a field.

## Code Migration

### Before
```rust
let parser = BeliefSetParser::new(...);
let accumulator = parser.accumulator();
let doc_local = accumulator.set();
let session_accumulated = accumulator.stack_cache();
```

### After
```rust
let compiler = DocumentCompiler::new(...);
let builder = compiler.graph_builder();
let doc_local = builder.document_graph();
let session_accumulated = builder.session_graph();
```

Note: Global persistent cache is accessed via the `global_cache` parameter.
```

## Testing Requirements

- [ ] All tests pass with new names
- [ ] No compilation errors or warnings
- [ ] Deprecation warnings work correctly (if using aliases)
- [ ] Examples compile and run
- [ ] Documentation builds without errors
- [ ] Manual review of all renamed items

## Success Criteria

- [ ] All types renamed consistently
- [ ] All fields renamed consistently
- [ ] All documentation updated
- [ ] Migration guide complete
- [ ] Type aliases in place (if gradual migration)
- [ ] No breaking changes for users (via aliases)
- [ ] Compiler analogy is clear in docs
- [ ] Field names are self-documenting

## Risks

**Risk**: Breaking change for existing users  
**Mitigation**: Use type aliases and deprecation warnings for gradual migration

**Risk**: Inconsistent naming across codebase  
**Mitigation**: Comprehensive grep/search for all usages; automated refactoring where possible

**Risk**: Documentation gets out of sync  
**Mitigation**: Update all docs in same PR; make this a single atomic change

**Risk**: Choosing wrong names  
**Mitigation**: Get feedback on naming proposal before implementing; easy to adjust with type aliases

## Open Questions

1. **Which naming option to choose?** (A, B, C, or D)
   - Recommendation: Option C (Hybrid) or D (Most Intuitive)
   - Need feedback from maintainers and early users

2. **Gradual or atomic migration?**
   - Gradual: Use type aliases, deprecate over multiple versions
   - Atomic: Rename everything in one release
   - Recommendation: Atomic (pre-1.0, can still break)

3. **Keep compatibility aliases forever?**
   - Could keep for v1.0+ compatibility
   - Or remove in v0.2.0 (since pre-1.0)
   - Recommendation: Remove by v1.0.0

4. **Rename related types too?**
   - `ProtoBeliefNode` → `ParsedNode`?
   - `BeliefEvent` → `GraphEvent`?
   - `BeliefNode` → keep as is?
   - Recommendation: Address in separate issue if needed

## Additional Improvements

Consider renaming in related areas:

### Codec Module
- `DocCodec` → `DocumentCodec` (clearer)
- `MdCodec` → `MarkdownCodec` (more explicit)
- `TomlCodec` → keep as is (TOML is standard)

### Properties Module
- `BeliefNode` → could be `GraphNode`, but "Belief" is established
- Keep as is for now

### Event Module
- `BeliefEvent` → could be `GraphEvent`
- Keep as is for now

## Timeline

- **Pre-v0.1.0**: Ideal time (before public announcement)
- **Before v1.0.0**: Acceptable time (pre-stability)
- **Post-v1.0.0**: Requires deprecation cycle

**Recommendation**: Complete before v0.1.0 announcement if possible, otherwise before v1.0.0.

## References

- **Design Doc**: `docs/design/beliefset_architecture.md` - Section 2.5, 3.1
- **Compilation Model**: `docs/design/beliefset_architecture.md` - Section 2.1
- **Rustdoc**: `src/lib.rs` - Architecture overview
- **Related Issues**: 
  - Issue 5 (Documentation) - will need updates
  - Future LSP work - naming matters for API exposure

## Decision Log

**To be filled during implementation**:
- Which naming option was chosen
- Rationale for final decision
- Any deviations from proposal
- Breaking vs non-breaking approach

---

**Status**: Proposed  
**Created**: 2025-01-17  
**Target**: Pre-v0.1.0 (optional but recommended) or Pre-v1.0.0 (required)