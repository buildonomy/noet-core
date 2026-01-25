# Issue 14: Pedagogical Naming Improvements

**Priority**: MEDIUM - Improves clarity and maintainability  
**Estimated Effort**: 2-3 days  
**Dependencies**: None (can proceed anytime, but better before v0.1.0)  
**Context**: Improve naming to match compiler architecture analogy and reduce confusion

## Summary

Several type and field names in noet-core are pedagogically confusing and don't clearly convey their purpose or role in the compilation pipeline. This issue proposes renaming key types and fields to better match compiler architecture conventions and improve code readability.

**Key Problems**:
1. `BeliefSetParser` doesn't parse - it orchestrates (like a build system)
2. `GraphBuilder` does both parsing AND linking - not just accumulation
3. Field names are backwards: `.set` is local, `.stack_cache` is global
4. `BeliefSet` vs `Beliefs` distinction is unclear
5. Names don't align with the compiler analogy we document

**Decision**: After analysis, we'll rename `BeliefSet` → `BeliefBase` to better capture its role as database-style infrastructure that applications build upon. The `.bb` abbreviation parallels the familiar `.db` pattern.

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

### 2. GraphBuilder (codec/mod.rs)

**Problem**: Name suggests it only accumulates, but it both parses files AND links references.

**What it does**:
- Parses files via DocCodec
- Maintains document stack for structure
- Resolves references (linking)
- Creates relations between nodes
- Publishes events

**Compiler analogy**: Semantic analyzer + linker

### 3. GraphBuilder.set

**Problem**: Misleading name - this is the LOCAL/CURRENT document's BeliefSet, not the accumulated result.

**What it is**: The BeliefSet for the currently-being-parsed document (document-scoped)

**Expected meaning**: The accumulated result (but it's not!)

### 4. GraphBuilder.stack_cache

**Problem**: Name doesn't convey that this is the SESSION-accumulated cache, not the global persistent cache.

**What it is**: The accumulated BeliefSet across documents parsed in this SESSION (in-memory, not persisted)

**Expected meaning**: Some temporary cache for the stack (but it's not!)

**Note**: The actual GLOBAL cache is the DB-backed `global_cache: B` parameter passed to the accumulator.

### 5. BeliefSet vs Beliefs

**Problem**: Distinction is unclear from names alone.

**What they are**:
- `BeliefSet`: Full-featured, indexed, queryable graph structure (like a database)
- `Beliefs`: Lightweight transport structure (just states + relations - raw graph data)

**Analysis**: "BeliefSet" undersells the richness - it's not just a set, it's a database-style structure with indexing, querying, and graph operations. The name should emphasize:
- Infrastructure that applications build upon
- Domain-specific storage with query semantics (like a database)
- Foundation for belief storage without prescribing application logic

## Three-Tier Caching Architecture

Understanding the three levels of scope is critical to understanding the confusing field names:

```
┌─────────────────────────────────────────────────────────────┐
│  Document-Scoped (.set → .doc_bb)                           │
│  - Currently-being-parsed document only                     │
│  - Cleared/reset between documents                          │
│  - BeliefBase for single file                               │
└─────────────────────────────────────────────────────────────┘
                         ↓ accumulates into
┌─────────────────────────────────────────────────────────────┐
│  Session-Scoped (.stack_cache → .session_bb)                │
│  - Accumulated across all documents in this parse session   │
│  - In-memory only, lost on process exit                     │
│  - BeliefBase for all parsed files                          │
└─────────────────────────────────────────────────────────────┘
                         ↓ syncs to/from
┌─────────────────────────────────────────────────────────────┐
│  Global/Persistent (global_cache: B parameter → global_bb)  │
│  - DB-backed, persists across sessions                      │
│  - Authoritative source for BID identity                    │
│  - Shared across all parsing sessions                       │
└─────────────────────────────────────────────────────────────┘
```

**The Problem**: Field names don't clearly indicate which scope they represent.

**The Solution**: Use `.bb` abbreviation (paralleling `.db` for database) with scope prefixes to make the three-tier architecture explicit.

## Proposed Naming Scheme

**Three Levels of Scope**:
1. **Document-local**: Current document being parsed (`.set` → `.doc_bb`)
2. **Session-local**: Accumulated in-memory during parsing session (`.stack_cache` → `.session_bb`)
3. **Global/persistent**: DB-backed cache passed as parameter (`global_cache: B` → `global_bb: B`)

**Rationale for BeliefBase**:
- Database analogy: BeliefBase is to beliefs what database is to data
- Infrastructure foundation: Applications build upon it without it prescribing logic
- Familiar abbreviation: `.bb` parallels `.db` convention
- Unique and recognizable: `.bb` is distinctive, unlikely to collide
- Domain-specific semantics: Like a database but with graph query semantics for beliefs

### SELECTED: Domain-Aware Database-Style Naming

**Rationale**: 
- `BeliefBase` captures the database-style infrastructure role
- Infrastructure types drop "Belief" prefix (they operate on beliefs but aren't beliefs)
- Field abbreviations use `.bb` (paralleling `.db`) with scope prefixes
- Maintains "Belief" terminology where it matters (domain concepts)

| Current | Proposed | Rationale |
|---------|----------|-----------|
| `BeliefSetParser` | `DocumentCompiler` | Infrastructure - orchestrates compilation |
| `BeliefSetAccumulator` | `GraphBuilder` | Infrastructure - builds graph from documents |
| `BeliefSet` | `BeliefBase` | Database-style belief storage with query semantics |
| `Beliefs` | `BeliefGraph` | Raw graph data (states + relations) |
| `BeliefCache` trait | `BeliefSource` trait | Abstraction for sources that provide belief data |
| `.set` | `.doc_bb` | Document-scoped BeliefBase |
| `.stack_cache` | `.session_bb` | Session-scoped BeliefBase |
| `global_cache: B` | `global_bb: B` | Global-scoped BeliefBase parameter |

**Why BeliefBase**:
1. **Database analogy**: BeliefBase : beliefs :: Database : data
2. **Infrastructure foundation**: Applications build upon it, it doesn't prescribe logic
3. **Familiar pattern**: `.bb` parallels `.db` (database) convention
4. **Unique abbreviation**: `.bb` is distinctive and unlikely to collide
5. **Semantic fit**: "Base" implies foundational, structural, durable
6. **Works with Intention Lattice**: BeliefBase is the general container; Intention Lattice is a lattice-structured interpretation of certain paths within it

### Alternative Options (Not Selected)

<details>
<summary>Option A: Traditional Compiler Terms</summary>

**Pros**: Clear analogy to compiler architecture  
**Cons**: Loses "Belief" domain terminology entirely

| Current | Proposed |
|---------|----------|
| `BeliefSet` | `IndexedGraph` |
| `Beliefs` | `GraphData` |
| `.set` | `.current_document` |
| `.stack_cache` | `.session_cache` |

</details>

<details>
<summary>Option B: Network-Based Naming</summary>

**Pros**: Emphasizes interconnection  
**Cons**: "Network" could imply distributed systems

| Current | Proposed |
|---------|----------|
| `BeliefSet` | `BeliefNetwork` |
| `Beliefs` | `BeliefGraph` |
| `.set` | `.doc_net` |
| `.stack_cache` | `.session_net` |

</details>

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

// In beliefset.rs
#[deprecated(note = "Use BeliefBase instead")]
pub type BeliefSet = BeliefBase;
```

### Phase 2: Rename Types (1 day)

1. **Rename struct definitions**:
   - [ ] `BeliefSetParser` → `DocumentCompiler`
   - [ ] `BeliefSetAccumulator` → `GraphBuilder`
   - [ ] `BeliefSet` → `BeliefBase`
   - [ ] `Beliefs` → `BeliefGraph`
   - [ ] `BeliefCache` trait → `BeliefSource` trait

2. **Update all usages**:
   - [ ] Update imports across codebase
   - [ ] Update method names that reference old types
   - [ ] Update error messages

3. **Update tests**:
   - [ ] Rename test helper types
   - [ ] Update test documentation

### Phase 3: Rename Fields (0.5 days)

1. **In GraphBuilder (formerly BeliefSetAccumulator)**:
   - [ ] `.set` → `.doc_bb` - document-scoped BeliefBase
   - [ ] `.stack_cache` → `.session_bb` - session-scoped BeliefBase
   - [ ] Add public accessors: `pub fn document_base(&self) -> &BeliefBase { &self.doc_bb }`
   - [ ] Add public accessors: `pub fn session_base(&self) -> &BeliefBase { &self.session_bb }`

2. **Update all field accesses**:
   - [ ] Search for `.set` usages (careful - common name!)
   - [ ] Search for `.stack_cache` usages
   - [ ] Update with new names (`.doc_bb`, `.session_bb`)
   - [ ] Update parameter names: `global_cache: B` → `global_bb: B`

**Note**: The `.bb` abbreviation parallels `.db` for database, making the three-tier scope (`.doc_bb`, `.session_bb`, `.global_bb`) immediately clear.

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
- `BeliefSet` → `BeliefBase`
- `Beliefs` → `BeliefGraph`
- `BeliefCache` trait → `BeliefSource` trait

## Field Renames

- `GraphBuilder.set` → `GraphBuilder.doc_bb` (document-scoped BeliefBase)
- `GraphBuilder.stack_cache` → `GraphBuilder.session_bb` (session-scoped BeliefBase)
- `global_cache: B` → `global_bb: B` (global-scoped BeliefBase parameter)

## Abbreviation Convention

The `.bb` abbreviation parallels `.db` (database) and emphasizes the three-tier scope:
- `.doc_bb` - document-scoped (current file being parsed)
- `.session_bb` - session-scoped (accumulated in-memory this session)
- `.global_bb` - global-scoped (persisted across sessions)

## Code Migration

### Before
```rust
let parser = BeliefSetParser::new(...);
let accumulator = parser.accumulator();
let doc_local: &BeliefSet = accumulator.set();
let session_accumulated: &BeliefSet = accumulator.stack_cache();

// Using trait bound
async fn query<B: BeliefCache>(cache: &B, expr: &Expression) {
    let result = cache.eval_unbalanced(expr).await;
}
```

### After
```rust
let compiler = DocumentCompiler::new(...);
let builder = compiler.graph_builder();
let doc_local: &BeliefBase = builder.document_base();
let session_accumulated: &BeliefBase = builder.session_base();

// Using trait bound
async fn query<B: BeliefSource>(source: &B, expr: &Expression) {
    let result = source.eval_unbalanced(expr).await;
}
```

### Internal Code (Direct Field Access)
```rust
// Before
self.set.states()
self.stack_cache.relations()

// After
self.doc_bb.states()
self.session_bb.relations()
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

1. **~~Which naming option to choose?~~** ✅ RESOLVED
   - **Decision**: BeliefBase with `.bb` abbreviation
   - **Rationale**: Database analogy, familiar `.db`-style pattern, unique abbreviation
   - See "SELECTED: Domain-Aware Database-Style Naming" section above

2. **Gradual or atomic migration?**
   - Gradual: Use type aliases, deprecate over multiple versions
   - Atomic: Rename everything in one release
   - Recommendation: Atomic (pre-1.0, can still break)

3. **Keep compatibility aliases forever?**
   - Could keep for v1.0+ compatibility
   - Or remove in v0.2.0 (since pre-1.0)
   - Recommendation: Remove by v1.0.0

4. **Rename related types too?**
   - `ProtoBeliefNode` → keep as is (still a belief node, just proto)
   - `BeliefEvent` → keep as is (events about beliefs)
   - `BeliefNode` → keep as is (domain concept)
   - `BeliefKind` → keep as is (domain concept)
   - `BeliefCache` trait → **BeliefSource** (abstraction for sources that provide belief data)
   - **Decision**: Keep "Belief" terminology for domain concepts; infrastructure and abstraction traits get clearer names

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

**2025-01-27: BeliefBase Decision**
- **Chosen naming**: BeliefBase with `.bb` abbreviation pattern
- **Rationale**: 
  - Database analogy: BeliefBase is infrastructure like a database
  - Familiar pattern: `.bb` parallels `.db` convention
  - Three-tier clarity: `.doc_bb`, `.session_bb`, `.global_bb` make scope explicit
  - Domain terminology preserved: "Belief" remains in domain concepts (BeliefNode, BeliefEvent, etc.)
  - Infrastructure distinction: Parser/Accumulator drop "Belief" prefix as they're build tools
  - Trait abstraction: BeliefCache → BeliefSource (clearer role as source of belief data)
- **Context**: Analyzed relationship to Intention Lattice (lattice-structured interpretation of paths within the general BeliefBase container)
- **Migration approach**: TBD - likely atomic for pre-1.0 simplicity
- **BeliefSource rationale**: Common pattern (DataSource, EventSource), concise, accurately describes trait's role as abstraction over different sources of belief data (in-memory or persistent)

---

**Status**: Proposed  
**Created**: 2025-01-17  
**Target**: Pre-v0.1.0 (optional but recommended) or Pre-v1.0.0 (required)
