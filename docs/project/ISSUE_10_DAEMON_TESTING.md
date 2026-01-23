# Issue 10: Compiler Service Testing & Library Pattern Extraction

**Priority**: CRITICAL - Blocks Issue 5 (Documentation)  
**Estimated Effort**: 2-3 days  
**Dependencies**: None (should be completed before Issue 5)  
**Context**: Part of [`ROADMAP_OPEN_SOURCE_NOET-CORE.md`](./ROADMAP_OPEN_SOURCE_NOET-CORE.md) Phase 3 - Code Quality & Testing

## Summary

Migrate `compiler.rs` to `watch.rs`, extract library patterns for file watching and database integration. The `compiler.rs` module was written before `parser.rs` and contains untested code. It uses product-specific language (`LatticeService`) for library patterns (`FileUpdateSyncer`, file watching, database sync) that should be documented and exposed as examples and as an executable that can be installed and run locally as a service or user-space executable. This issue determines what belongs in the library vs. product, creates working tests/examples/executables, and prepares these patterns for Issue 5 documentation.

## Goals

1. Migrate `compiler.rs` → `watch.rs` (rename `LatticeService` → `WatchService`)
2. Establish library vs. product boundary for service components
3. Test `FileUpdateSyncer` with file watching integration
4. Test database synchronization via `perform_transaction`
5. Create `bin/noet.rs` CLI tool with subcommands:
   - `noet parse <path>` - one-shot parsing with diagnostics printed to stdout
   - `noet watch <path>` - continuous parsing (foreground) with diagnostics written to logfile
6. Write tutorial documentation with doctests in `watch.rs` module
7. Create `examples/watch_service.rs` demonstrating full orchestration
8. Extract library-appropriate operations from `commands.rs` (migrate from lattice_service crate)
9. Provide tested, documented code ready for Issue 5 documentation

## Architecture

### Current Structure (`src/compiler.rs`)

**LatticeService** (lines 41-329):
- `new()` - initializes runtime, db, config, codecs
- `get_networks()` / `set_networks()` - network management
- `get_focus()` / `set_focus()` - focus management (PRODUCT-SPECIFIC)
- `enable_belief_network_syncer()` - sets up file watcher + parser
- `disable_belief_network_syncer()` - tears down watcher
- `get_content()` / `set_content()` - content access
- `get_states()` - query interface

**FileUpdateSyncer** (lines 331-472):
- Spawns two async tasks: parser thread + transaction thread
- Parser thread: continuously processes `BeliefSetParser` queue
- Transaction thread: batches `BeliefEvent`s and syncs to database
- Coordinates file watching → parsing → database sync pipeline

**Supporting**:
- `BnWatchers` - manages multiple file watchers
- `PaginationCache` - caches query results
- `perform_transaction()` - batches belief events to database

### Target Structure

**Module**: `src/watch.rs` (renamed from `compiler.rs`)
- `WatchService` (renamed from `LatticeService`) - orchestration layer
- `FileUpdateSyncer` - continuous parsing + file watching
- `perform_transaction()` - event batching

**Binary**: `src/bin/noet.rs` - CLI tool
```
noet parse <path>              # One-shot parse with diagnostics
noet watch <path>              # Continuous foreground parsing
noet serve [--config]          # (Future: ISSUE_11) Background service
```

**Examples**: `examples/watch_service.rs` - full orchestration demonstration

**Tutorial Documentation**: Doctests in `src/watch.rs` module doc

### Library vs. Product Boundary

**Library** (keep in noet-core):
- ✅ `WatchService` orchestration (rename from `LatticeService`)
- ✅ `FileUpdateSyncer` - file watching + continuous parsing
- ✅ `perform_transaction()` - event batching and database sync
- ✅ `get_networks()` / `set_networks()` - network configuration (library feature)
- ✅ `get_states()` - query interface (library feature)
- ✅ `PaginationCache` - query optimization (library feature)
- ✅ Operations from `commands.rs`: `LoadNetworks`, `SetNetworks`, `GetStates`, `UpdateContent`

**Product** (remove or don't migrate):
- ❌ `get_focus()` / `set_focus()` - motivation tracking (product-specific)
- ❌ `GetProc` / `SetProc` with `AsRun` - procedure execution (product-specific)
- ❌ `GetNetFromDir` with dialog - UI-specific operation

**Decision**: Service functionality IS library infrastructure. Users should be able to run their own watch services.

## Implementation Steps

1. **Establish Library vs. Product Boundary** (0.5 days)
   - [x] Review `LatticeService` methods - identify product-specific operations
   - [x] Decision: `get_focus()`/`set_focus()` are product-specific, don't migrate
   - [x] Review `commands.rs` from lattice_service crate
   - [x] Extract library operations: `LoadNetworks`, `SetNetworks`, `GetStates`
   - [x] Generalize `SetProc` → `UpdateContent` (remove product-specific `AsRun`)
   - [x] Remove product operations: `GetProc`, `GetFocus`, `SetFocus`, `GetNetFromDir`
   - [x] Document decision: Service is library infrastructure, users can run their own

2. **Migrate compiler.rs → watch.rs** (0.5 days)
   - [x] Rename file: `src/compiler.rs` → `src/watch.rs`
   - [x] Rename type: `LatticeService` → `WatchService`
   - [x] Update module exports in `src/lib.rs`
   - [x] Remove `get_focus()` / `set_focus()` methods (product-specific)
   - [x] Rename method: `enable_belief_network_syncer()` → `enable_network_syncer()`
   - [x] Rename method: `disable_belief_network_syncer()` → `disable_network_syncer()`
   - [x] Update all internal references and imports
   - [x] Keep feature flag as `#[cfg(feature = "service")]`

3. **Migrate and Refine Commands** (0.5 days)
   - [x] Review `commands.rs` from lattice_service crate
   - [x] Migrate library operations to `src/commands.rs`:
     - `Op::LoadNetworks` / `OpResult::Networks`
     - `Op::SetNetworks` / `OpResult::Networks`
     - `Op::GetStates` / `OpResult::Page`
     - `Op::UpdateContent` (generalized from `SetProc`)
   - [x] Remove product-specific operations from migration
   - [ ] Update `WatchService` to implement library operations
   - [ ] Document operation semantics in rustdoc

4. **Test FileUpdateSyncer in Isolation** (1 day)
   - [ ] Create test: Initialize `FileUpdateSyncer` with temp directory
   - [ ] Create test: Modify file, verify parser thread processes it
   - [ ] Create test: Verify `BeliefEvent`s flow to transaction thread
   - [ ] Create test: Verify database sync completes
   - [ ] Create test: Multiple file changes, verify all processed
   - [ ] Create test: Handle parse errors gracefully
   - [ ] Create test: Shutdown and cleanup (abort handles)
   - [ ] Document threading model and synchronization points in module doc
   - **Note**: Integration test skeleton created at `tests/service_integration.rs`

5. **Test File Watching Integration** (0.5 days)
   - [ ] Create test: `enable_network_syncer()` sets up watcher
   - [ ] Create test: File modification triggers debouncer callback
   - [ ] Create test: Debouncer filters dot files correctly
   - [ ] Create test: Debouncer filters by codec extensions
   - [ ] Create test: Parser queue gets updated on file change
   - [ ] Create test: `disable_network_syncer()` tears down cleanly
   - [ ] Verify no race conditions between debouncer and parser thread

6. **Test Database Synchronization** (0.5 days)
   - [ ] Create test: `perform_transaction()` batches multiple events
   - [ ] Create test: Events update database correctly
   - [ ] Create test: Transaction errors are handled gracefully
   - [ ] Create test: Event channel backpressure (if applicable)
   - [ ] Verify database state matches parser cache after sync
   - [ ] Document transaction boundaries and consistency guarantees

7. **Create CLI Tool: noet binary** (1 day)
   - [x] Create `src/bin/noet.rs` - CLI entry point using `clap`
   - [x] Implement `noet parse <path>` subcommand:
     - One-shot parse using `BeliefSetParser::simple()`
     - Display parser statistics
     - Exit code based on error count (TODO: improve diagnostics display)
   - [x] Implement `noet watch <path>` subcommand:
     - Initialize `WatchService` in foreground
     - Enable network syncer for path
     - Print file change events and parse results
     - Graceful shutdown on Ctrl-C
   - [x] Add `--verbose` / `--quiet` flags
   - [x] Add `--config <path>` flag for watch config
   - [x] Test `noet parse` subcommand with example documents (working!)
   - [ ] Test `noet watch` subcommand with example documents

8. **Write Tutorial Documentation with Doctests** (1 day)
   - [ ] Add module-level doc to `src/daemon.rs` with tutorial sections:
     - Overview: What is the daemon, when to use it
     - Quick Start: Minimal working example
     - File Watching Pattern: Manual file watcher setup
     - Database Sync Pattern: Event batching and persistence
     - Full Orchestration: Using `DaemonService`
     - CLI Tool: Using `noet parse` and `noet watch`
   - [ ] Convert all code examples to doctests (```rust blocks)
   - [ ] Verify doctests compile and run with `cargo test --doc`
   - [ ] Link from `lib.rs` rustdoc to daemon tutorial
   - [ ] Document threading model, synchronization, shutdown semantics

9. **Create Complete Example: daemon.rs** (0.5 days)
   - [ ] Create `examples/daemon.rs` demonstrating full orchestration:
     - Initialize `DaemonService`
     - Enable multiple network syncers
     - Query graph state
     - Handle events
     - Graceful shutdown
   - [ ] Add extensive inline comments explaining each step
   - [ ] Verify example compiles and runs
   - [ ] Reference from daemon module tutorial docs

## Testing Requirements

**Unit Tests**:
- `FileUpdateSyncer::new()` initializes correctly
- Parser thread processes queue continuously
- Transaction thread batches events correctly
- Shutdown aborts threads cleanly
- `WatchService` methods work correctly (rename from `LatticeService`)
- Command operations execute without product dependencies

**Integration Tests**:
- End-to-end: file modification → parse → database sync
- Multiple files in parallel
- Error recovery and resilience
- Memory cleanup on shutdown
- CLI tool subcommands work correctly

**Doctest Verification**:
- All tutorial code examples in `src/watch.rs` compile and run
- `cargo test --doc` passes without errors
- Examples demonstrate patterns clearly without external dependencies

**CLI Verification**:
- `noet parse <path>` compiles and runs, shows diagnostics
- `noet watch <path>` compiles and runs, watches files
- Error handling works (invalid paths, parse errors)
- Ctrl-C shutdown is graceful

**Example Verification**:
- `examples/watch_service.rs` compiles and runs
- Example is self-contained (no product dependencies)
- Example demonstrates full orchestration clearly

## Success Criteria

- [x] `compiler.rs` successfully migrated to `watch.rs`
- [x] `LatticeService` renamed to `WatchService`, product methods removed
- [x] Library operations extracted from `commands.rs` and integrated
- [ ] All tests pass for `FileUpdateSyncer` and `WatchService`
- [ ] File watching integration tested and working
- [ ] Database synchronization tested and working
- [x] CLI tool (`noet parse`, `noet watch`) implemented
- [ ] CLI tool fully tested (parse works, watch needs testing)
- [ ] Tutorial documentation with doctests in `src/watch.rs` compiles and passes
- [ ] `examples/watch_service.rs` demonstrates full orchestration
- [ ] Clear library vs. product boundary documented
- [ ] Module documentation clarifies component purposes
- [ ] Threading model and synchronization fully documented
- [ ] No blocking issues for Issue 5 documentation work

## Risks

**Risk**: Product-specific methods difficult to separate from service orchestration  
**Mitigation**: Clear documentation of what's library vs. product; remove `get_focus`/`set_focus` but keep orchestration layer

**Risk**: `FileUpdateSyncer` threading model has race conditions  
**Mitigation**: Add explicit tests for concurrent access; document synchronization points; consider refactoring if issues found

**Risk**: Database sync has undocumented consistency requirements  
**Mitigation**: Review transaction boundaries; document guarantees; add tests verifying consistency

**Risk**: CLI tool scope creep (too many features)  
**Mitigation**: Start minimal (`parse`, `watch` only); defer advanced features to ISSUE_11 (background service mode, REST API)

**Risk**: Doctests become too complex or fail intermittently  
**Mitigation**: Keep doctests focused on single concepts; use `no_run` for long-running examples; test thoroughly

**Risk**: Renaming breaks existing code  
**Mitigation**: This is pre-1.0, breaking changes acceptable; update all internal references; document migration in CHANGELOG

## Open Questions

1. **Should `WatchService` be public API or example-only?**
   - **Decision**: Public API - users should be able to run their own watch services
   - Mark as "advanced usage" in documentation
   - Keep behind `service` feature flag

2. **Is database synchronization a core library feature?**
   - **Decision**: Yes - it's a key pattern for maintaining persistent state
   - Needs comprehensive testing and documentation
   - Required for service functionality

3. **Should we expose `FileUpdateSyncer` as public API?**
   - **Decision**: Keep `pub(crate)` for now
   - Users interact via `WatchService` or manual patterns in doctests
   - Can promote to public in future if needed

4. **CLI tool: Single binary or multiple?**
   - **Decision**: Single binary with subcommands (`noet parse`, `noet watch`)
   - Follows pattern of `cargo`, `git`, `rustup`
   - Easier to install and discover functionality

5. **Feature flag strategy?**
   - Keep `service` feature flag (already appropriate)
   - Make service optional (not default) to keep core library minimal
   - Document feature flag requirements in README

## Decision Log

**Decision 1: Service is Library Infrastructure**
- Date: 2025-01-23
- Rationale: Users should be able to run their own watch services for continuous parsing and synchronization
- Impact: `WatchService` remains in noet-core as public API behind `service` feature flag

**Decision 2: Remove Product-Specific Operations**
- Date: 2025-01-23
- Removed: `get_focus()`, `set_focus()`, `GetProc`, `SetProc`, `GetFocus`, `SetFocus`, `GetNetFromDir`
- Kept: `LoadNetworks`, `SetNetworks`, `GetStates`, generalized `UpdateContent`
- Rationale: These operations reference product-specific schemas and UI concerns
- Impact: Cleaner library/product boundary, simpler API surface

**Decision 3: CLI Tool as Single Binary**
- Date: 2025-01-23
- Pattern: Single `noet` binary with subcommands
- Initial subcommands: `parse`, `watch`
- Future subcommands (ISSUE_11): `serve start/stop/status`, `query`, `check`
- Rationale: Better UX, follows Rust ecosystem patterns
- Implementation: Uses `clap` v4.5 with derive macros

**Decision 4: Tutorial Docs with Doctests**
- Date: 2025-01-23
- Approach: Module-level documentation with extensive doctest examples
- Keep `examples/watch_service.rs` for complete program demonstration
- Rationale: Doctests ensure examples stay synchronized with API changes
- Status: Deferred to next step (focus on testing first)

## References

- **Blocks**: [`ISSUE_05_DOCUMENTATION.md`](./ISSUE_05_DOCUMENTATION.md) - needs working service examples and tutorial docs
- **Roadmap Context**: [`ROADMAP_OPEN_SOURCE_NOET-CORE.md`](./ROADMAP_OPEN_SOURCE_NOET-CORE.md) - Phase 1 (updated to include ISSUE_10)
- **Follow-up**: ISSUE_11 (Future) - REST/IPC API, background service client/server mode
- **Code to migrate**: 
  - `src/compiler.rs` → `src/watch.rs` (rename entire module)
  - `rust_core/crates/lattice_service/src/commands.rs` → `src/commands.rs` (extract library operations)
- **Pattern**: `src/codec/parser.rs` - `BeliefSetParser` integration points
- **Dependencies**: 
  - `notify-debouncer-full` - file watching
  - `tokio` - async runtime and task spawning
  - `clap` - CLI argument parsing (new dependency)
  - `src/db/mod.rs` - database connection and transactions
  - `src/event.rs` - `BeliefEvent` definitions
- **Deliverables**:
  - `src/watch.rs` - ✅ migrated and renamed module (tutorial docs pending)
  - `src/bin/noet.rs` - ✅ CLI tool with `parse` and `watch` subcommands
  - `examples/watch_service.rs` - ⏳ complete orchestration example (TODO)
  - `tests/service_integration.rs` - ✅ integration test skeleton created
  - Tutorial documentation with doctests in module doc (TODO)
- **Future Work (ISSUE_11)**:
  - REST/IPC API layer (JSON-RPC or LSP protocol)
  - `noet serve start/stop/status` subcommands
  - Client/server architecture with event streaming
  - Additional CLI commands: `query`, `check`, `inject-bids`
