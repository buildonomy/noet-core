# Issue 95: Workspace Decomposition for Build Time

**Priority**: MEDIUM
**Estimated Effort**: 5 days (RELATIVE COMPARISON ONLY)
**Dependencies**: None

## Summary

noet-core is a 70k LoC single-crate monolith. Every edit recompiles the entire
codebase — `cargo check` routinely exceeds 5 minutes on capable hardware. Splitting
into a Cargo workspace with focused crates would allow incremental compilation to
parallelize across crate boundaries, dramatically reducing edit-check cycle time.

## Goals

- Reduce `cargo check` time after a single-file edit from minutes to seconds
- Preserve the existing public API surface (`noet-core` becomes a thin re-export crate)
- Enable parallel compilation across independent subsystems

## Architecture

### Current Structure (single crate)

```
noet-core/src/
  lib.rs             ← one compilation unit
  beliefbase/        ← graph data structures, event processing
  codec/             ← parsers, compiler, builder (largest modules)
  query/             ← query spec, parser, evaluation
  paths/             ← PathMap, AnchorPath
  shard/             ← export, search index, content types
  mcp/               ← MCP server
  properties.rs      ← Bid, Weight, BeliefKind, constants
  event.rs           ← BeliefEvent enum
  nodekey.rs         ← NodeKey enum
  error.rs           ← BuildonomyError
  db.rs              ← database layer
  wasm.rs            ← WASM bindings
  cli.rs, commands.rs, watch.rs, dev_server.rs  ← CLI/runtime
```

### Proposed Workspace

Split along existing module boundaries, bottom-up by dependency order:

| Crate | Contents | Approx LoC |
|-------|----------|-----------|
| `noet-types` | `properties`, `event`, `nodekey`, `error` | ~5k |
| `noet-beliefbase` | `beliefbase/`, `paths/` | ~11k |
| `noet-query` | `query/` | ~6k |
| `noet-codec` | `codec/` (parsers, builder, compiler, proto_index) | ~30k |
| `noet-shard` | `shard/`, `db` | ~5k |
| `noet-mcp` | `mcp/` | ~2k |
| `noet-core` | Re-export facade, `cli`, `commands`, `watch`, `wasm`, `layout` | ~10k |

Dependency graph (each crate depends only on those above it):

```
noet-types
  └─ noet-beliefbase
       └─ noet-query
            └─ noet-codec
                 └─ noet-shard
                      └─ noet-core (facade + CLI)
  └─ noet-mcp (depends on noet-query + noet-beliefbase)
```

A change to `compiler.rs` (in `noet-codec`) recompiles only `noet-codec` +
`noet-shard` + `noet-core` — not `noet-types`, `noet-beliefbase`, or `noet-query`.

### Contributing Factors to Current Build Time

| Factor | Impact |
|--------|--------|
| Single crate, 70k LoC | No incremental parallelism |
| 321 derive macros (mostly serde) | Substantial codegen per struct |
| 989 transitive dependencies | First build is brutal |
| Heavy generics (`B: BeliefSource + Clone + Send + 'static`) | Monomorphized per call site |
| Large async functions (500+ line state machines) | Proportional to function size |

## Implementation Steps

1. Create workspace root `Cargo.toml` with `[workspace]` (~0.5 day)
   - [ ] Set up workspace members
   - [ ] Move shared dependency versions to `[workspace.dependencies]`

2. Extract `noet-types` (~0.5 day)
   - [ ] Move `properties.rs`, `event.rs`, `nodekey.rs`, `error.rs`
   - [ ] Fix all `use crate::` → `use noet_types::` in downstream

3. Extract `noet-beliefbase` (~1 day)
   - [ ] Move `beliefbase/` and `paths/`
   - [ ] Depends on `noet-types`

4. Extract `noet-query` (~0.5 day)
   - [ ] Move `query/`
   - [ ] Depends on `noet-types` + `noet-beliefbase`

5. Extract `noet-codec` (~1.5 days)
   - [ ] Move `codec/`
   - [ ] This is the largest and most interconnected module
   - [ ] Depends on `noet-types` + `noet-beliefbase` + `noet-query`

6. Extract remaining crates and wire up facade (~1 day)
   - [ ] `noet-shard`, `noet-mcp`
   - [ ] `noet-core` becomes a thin re-export + CLI

## Testing Requirements

- All existing tests pass (they move with their modules)
- `cargo check` after editing `src/codec/compiler.rs` completes in < 30s

## Success Criteria

- [ ] `cargo check` for a single-file edit in `noet-codec` < 30 seconds
- [ ] Full clean build time is not significantly worse than current
- [ ] Public API unchanged (re-exported from `noet-core`)
- [ ] All CI tests pass

## Risks

- **Circular dependencies**: `codec` ↔ `beliefbase` coupling may require
  trait extraction to break cycles → **Mitigation**: audit `use crate::` graph
  before starting; extract shared traits into `noet-types`
- **Test fixture sharing**: integration tests spanning multiple modules may need
  a `noet-test-support` crate → **Mitigation**: identify shared test helpers
  early in step 1
- **WASM target**: `wasm.rs` touches many modules; may need careful feature
  gating → **Mitigation**: keep wasm bindings in the facade crate

## Open Questions

- Should `noet-types` include `BeliefNode` / `BeliefGraph` (currently in
  `beliefbase`), or do those belong in `noet-beliefbase`? The answer depends on
  whether `noet-query` needs to reference them without depending on
  `noet-beliefbase`.
- Is the `codec` ↔ `beliefbase` boundary clean enough to split without
  significant refactoring? A quick `grep` for cross-module imports would
  answer this.
