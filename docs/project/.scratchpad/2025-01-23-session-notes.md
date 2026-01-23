# SCRATCHPAD - NOT DOCUMENTATION

**Date**: 2025-01-23  
**Purpose**: Working notes for ISSUE_10, ISSUE_15, ISSUE_16 completion  
**Delete When**: Session complete or notes no longer useful

## Session Context

Working on v0.1.0 roadmap, currently in Phase 1 (CLI and Watch Service).

### Completed Today

1. **ISSUE_10: Service Migration (60% complete)**
   - ✅ Renamed `compiler.rs` → `watch.rs`
   - ✅ Renamed `LatticeService` → `WatchService`
   - ✅ Removed product methods: `get_focus()`, `set_focus()`
   - ✅ Cleaned up `commands.rs` (removed GetProc, SetProc, GetFocus, etc.)
   - ✅ Created `bin/noet.rs` with `parse` and `watch` subcommands
   - ✅ `noet parse` command works! Tested successfully
   - ✅ Created test skeleton: `tests/service_integration.rs`

2. **ISSUE_15: Filtered Event Streaming (new, 405 lines)**
   - Query-based event subscriptions
   - Bidirectional streaming (client ↔ server)
   - Focus concept: query scope defines read/write permissions
   - Added to v0.3.0 roadmap

3. **ISSUE_16: Distributed Event Log (new, 716 lines - REVISED)**
   - **Key insight**: Not state sync, it's event sourcing
   - Multiple producers → Automerge rotated logs → SQLite indices → queries
   - Focus-based authorization via Keyhive
   - Integrates with procedure engine (action detection, redline learning)
   - Added to v0.4.0 roadmap

### Key Architecture Decisions

**ISSUE_16 Revision** (important!):
- Original: "Sync activity logs across devices" → SQLite is fine for this
- Revised: "Distributed event log for procedure matching" → Automerge solves hard problems
- Reason: User needs chronological merging with Lamport clocks, focus-based auth, cloud archival
- Automerge used for: Rotated event log files (canonical, append-only)
- SQLite used for: Derivative indices (fast queries, rebuilt from Automerge)

**Focus Concept** (spans ISSUE_15 and ISSUE_16):
- Focus = query scope (PaginatedQuery)
- Focus = permission boundary (Keyhive capabilities)
- User + Focus → read/write permissions
- Example: "focus-work-lsp" = all events related to ISSUE_11

### Remaining Work for ISSUE_10

**High Priority (Next Session)**:
1. Test `noet watch` command (manual testing with live files)
2. Implement integration tests (remove `#[ignore]` from test skeleton)
3. Create `examples/watch_service.rs` (full orchestration demo)
4. Write tutorial docs with doctests in `watch.rs` module

**Testing Checklist**:
- [ ] `noet watch` tested with file modifications
- [ ] FileUpdateSyncer tests pass
- [ ] Database sync tests pass
- [ ] CLI graceful shutdown (Ctrl-C) works
- [ ] Doctests compile and run

### Cross-References to Verify

- ISSUE_10 references ISSUE_15 (filtered streaming)
- ISSUE_15 references ISSUE_10 (WatchService foundation)
- ISSUE_16 references ISSUE_10 and ISSUE_15
- ROADMAP_NOET-CORE_v0.1.md Phase 1 updated
- ROADMAP.md v0.3.0 includes ISSUE_15
- ROADMAP.md v0.4.0 includes ISSUE_16

### Build Status

- ✅ `cargo check --features service` passes
- ✅ `cargo build --features bin` succeeds
- ✅ `noet parse /tmp/noet_test` works correctly
- ⏳ `noet watch` needs testing
- ⏳ Integration tests need implementation

### Feature Flags

```toml
[features]
default = ["bin"]
bin = ["service", "clap", "ctrlc", "tracing-subscriber"]
service = ["notify", "serde_json", "sqlx", "notify-debouncer-full", "futures-core"]
```

### Dependencies Added

- `clap` v4.5 (CLI parsing)
- `ctrlc` v3.4 (graceful shutdown)
- `tracing-subscriber` v0.3 (logging)

### Files Created/Modified

**Created**:
- `src/bin/noet.rs` - CLI tool
- `tests/service_integration.rs` - Test skeleton (15+ tests, all `#[ignore]`)
- `docs/project/ISSUE_15_FILTERED_EVENT_STREAMING.md`
- `docs/project/ISSUE_16_AUTOMERGE_INTEGRATION.md` (revised)
- `docs/project/.scratchpad/` directory with README

**Modified**:
- `src/watch.rs` (renamed from compiler.rs)
- `src/lib.rs` (module exports)
- `src/commands.rs` (removed product ops)
- `Cargo.toml` (dependencies, features)
- `docs/project/ISSUE_10_DAEMON_TESTING.md` (progress updates)
- `docs/project/ROADMAP_NOET-CORE_v0.1.md` (Phase 1)
- `docs/project/ROADMAP.md` (v0.3.0, v0.4.0)
- `AGENTS.md` (added scratchpad guidelines)

### Questions for Next Session

1. Start with testing `noet watch` or implementing integration tests?
2. Should examples come before or after tests?
3. Any changes needed to CLI based on testing?

### Notes

- Terminology settled: "WatchService" (not daemon/service)
- All core refactoring done, foundation is solid
- Ready for testing phase
- HTML rendering (Phase 2) waits until after ISSUE_10 complete

---

**Remember**: Delete this file when session ends or notes no longer useful.