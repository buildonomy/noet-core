# Issue 19: File Watcher Timing Bug Investigation

**Priority**: HIGH
**Estimated Effort**: 1-2 days
**Dependencies**: None (blocks Issue 5 indirectly - affects CLI reliability)

## Summary

The file watcher integration test (`test_file_modification_triggers_reparse`) is currently marked `#[ignore]` due to timing sensitivity. After a 7-second wait, the test receives 0 events when it should receive multiple `Event::Belief` updates after file modification. This suggests a potential bug in the file watcher → parser → event emission pipeline rather than just test flakiness.

**Critical concern**: If this is a real bug (not just test timing), it means `noet watch` CLI command may not work correctly, which would block soft open source release.

## Goals

1. Determine if file watcher timing issue is a real bug or test environment artifact
2. If bug: Identify root cause in watcher → parser → event emission chain
3. If bug: Fix and verify with reliable test
4. If test artifact: Document why test is unreliable and create manual verification procedure
5. Verify `noet watch` CLI works correctly with real file changes
6. Update integration test to be reliable (not ignored)

## Current Behavior

**Test code** (`tests/service_integration.rs:94-150`):
```rust
#[test]
#[ignore = "File watching can be timing-sensitive in test environments"]
fn test_file_modification_triggers_reparse() {
    // ... setup ...
    service.enable_network_syncer(&network_path).unwrap();
    
    // Wait for initial parse
    std::thread::sleep(Duration::from_secs(3));
    
    // Drain initial events
    while rx.try_recv().is_ok() {}
    
    // Modify file
    std::fs::write(&doc_path, updated_content).unwrap();
    
    // Wait for file watcher debouncer and reparse
    std::thread::sleep(Duration::from_secs(7));  // ← 7 seconds!
    
    // Verify we received events
    let mut event_count = 0;
    while rx.try_recv().is_ok() {
        event_count += 1;
    }
    
    assert!(event_count > 0);  // ← FAILS: event_count = 0
}
```

**Expected**: After 7 seconds, should receive multiple `Event::Belief` updates
**Actual**: Receives 0 events
**Status**: Test passes with `#[ignore]`, fails when run

## Architecture Review

### File Watcher Pipeline

```
File System
    ↓ (notify crate)
File Watcher Thread (notify-debouncer-full)
    ↓ (300ms debounce)
Debouncer Callback
    ↓ (enqueue modified path)
Parser Thread (FileUpdateSyncer::parser_handle)
    ↓ (parse and emit BeliefEvents)
Transaction Thread (FileUpdateSyncer::transaction_handle)
    ↓ (batch and forward)
Main Thread (mpsc::Receiver<Event>)
```

### Potential Failure Points

1. **File watcher not triggering**
   - OS-specific notification delays
   - Debouncer not invoking callback
   - Filter excluding modified file

2. **Parser queue not processing**
   - Parser thread blocked/panicked
   - Queue not receiving paths
   - Parse errors causing silent failures

3. **Event channel broken**
   - Transaction thread not forwarding events
   - Channel disconnected
   - Events emitted but not received

4. **Test timing assumptions wrong**
   - 7 seconds insufficient for full pipeline
   - Initial parse still running when test modifies file
   - Race condition in event draining

## Investigation Steps

### Step 1: Manual CLI Testing (0.5 days)

**Critical first step**: Verify if `noet watch` actually works in real usage.

```bash
# Create test directory
mkdir -p /tmp/noet_test/network
cd /tmp/noet_test/network

# Create BeliefNetwork.toml
cat > BeliefNetwork.toml << EOF
id = "test-network"
title = "Test Network"
EOF

# Create initial document
cat > doc1.md << EOF
# Document 1

Initial content.
EOF

# Start watching
cargo run --features service --bin noet -- watch /tmp/noet_test

# In another terminal, modify doc1.md
echo "# Document 1\n\nModified content." > /tmp/noet_test/network/doc1.md

# Observe: Does noet watch output show reparse?
```

**Success criteria**:
- [ ] `noet watch` detects file change within 1-2 seconds
- [ ] Console output shows "Parsing..." or similar
- [ ] Database updated with new content
- [ ] No errors or warnings

**If this fails**: Real bug, proceed to Step 2
**If this succeeds**: Test environment issue, proceed to Step 3

### Step 2: Debug File Watcher Pipeline (1 day, if Step 1 fails)

Add detailed logging to trace event flow:

1. **Add tracing to debouncer callback** (`src/watch.rs`)
   ```rust
   // In enable_network_syncer, debouncer callback
   tracing::info!("File watcher detected change: {:?}", event.paths);
   ```

2. **Add tracing to parser queue** (`src/watch.rs`, FileUpdateSyncer)
   ```rust
   tracing::info!("Parser enqueuing: {:?}", path);
   tracing::info!("Parser processing: {:?}", path);
   ```

3. **Add tracing to event emission** (transaction thread)
   ```rust
   tracing::info!("Transaction thread received event: {:?}", event);
   tracing::info!("Forwarding event to main channel");
   ```

4. **Run test with tracing**
   ```bash
   RUST_LOG=noet_core=trace cargo test --features service test_file_modification_triggers_reparse -- --ignored --nocapture
   ```

5. **Analyze logs**: Where does the pipeline break?
   - Watcher triggers? (if not: OS notification issue)
   - Parser enqueues? (if not: debouncer callback broken)
   - Parser processes? (if not: parser thread issue)
   - Events emitted? (if not: transaction thread issue)
   - Events received? (if not: channel issue)

### Step 3: Fix Test Environment Issues (0.5 days, if Step 1 succeeds)

If manual testing works but automated test fails:

**Option A: Use inotify-rs test patterns**
- Look at notify-debouncer-full's own tests
- May need to use `notify::Config` with specific settings
- Possible platform-specific test configuration

**Option B: Mock file watcher for testing**
- Create `MockWatcher` that directly calls parser queue
- Test pipeline without OS file system notifications
- Keep current test as manual verification only

**Option C: Integration test with longer waits**
- Increase wait time to 10-15 seconds
- Add retry logic with timeout
- Accept that file system tests are inherently flaky

**Option D: Use temporary directory utilities**
- Some OSes (especially in CI) have slow FS notifications
- Try different temp directory backends
- Check if running in container affects notifications

### Step 4: Verify Fix (0.5 days)

After fix implemented:

1. **Run integration test 20 times**
   ```bash
   for i in {1..20}; do
       cargo test --features service test_file_modification_triggers_reparse -- --ignored
       if [ $? -ne 0 ]; then
           echo "Failed on iteration $i"
           exit 1
       fi
   done
   ```

2. **Test on multiple platforms**
   - Linux (primary target)
   - macOS (if available)
   - Windows (if applicable)

3. **Document platform-specific behavior** if any

## Testing Requirements

- [ ] Manual CLI test passes (file changes trigger reparse within 2 seconds)
- [ ] Integration test passes reliably (>95% success rate over 20 runs)
- [ ] Test no longer marked `#[ignore]`
- [ ] Documented any platform-specific limitations
- [ ] CI environment tested (if file system notifications work in CI)

## Success Criteria

- [ ] Root cause identified (bug vs. test artifact)
- [ ] If bug: Fixed with test demonstrating reliability
- [ ] If test artifact: Documented why, with manual verification procedure
- [ ] `noet watch` CLI verified to work correctly with real files
- [ ] Integration test either fixed or replaced with reliable alternative
- [ ] No blocking issues for soft open source release

## Risks

**Risk**: This is a fundamental feature (file watching) - if broken, `noet watch` is unusable
**Mitigation**: Prioritize manual CLI testing first to determine severity

**Risk**: File system notifications are inherently OS-dependent and timing-sensitive
**Mitigation**: Accept some tests may need to be manual verification only

**Risk**: Fix may require redesigning threading model
**Mitigation**: Time-box investigation to 2 days, then defer if too complex

**Risk**: May be notify-debouncer-full library bug
**Mitigation**: Check library issues, consider pinning version or workaround

## Open Questions

1. **Does `noet watch` CLI actually work in manual testing?**
   - If yes: Test environment issue only
   - If no: Critical bug blocking soft open source

2. **Which thread/component is the bottleneck?**
   - File watcher thread?
   - Parser thread?
   - Transaction thread?
   - Event channel?

3. **Is 300ms debounce too aggressive?**
   - Should it be configurable?
   - Does test need longer wait for debounce + parse + transaction?

4. **Is this OS-specific?**
   - Linux inotify vs. macOS FSEvents vs. Windows ReadDirectoryChangesW
   - Test environment (container, VM, CI) affecting notifications?

5. **Are there existing issues in notify-debouncer-full?**
   - Check: https://github.com/notify-rs/notify/issues
   - Version: currently using notify-debouncer-full v0.3.1

## References

- **Blocks**: None directly, but affects `noet watch` CLI reliability
- **Related**: Issue 10 (WatchService implementation) - this tests what Issue 10 built
- **Related**: Issue 5 (Documentation) - need working CLI for examples
- **Code**: `src/watch.rs` (FileUpdateSyncer, enable_network_syncer)
- **Test**: `tests/service_integration.rs:94-150`
- **Dependencies**: 
  - `notify-debouncer-full` v0.3.1
  - `notify` v6.1.1
- **Similar issues**: Check notify-rs/notify repository for timing issues

## Decision Log

**Decision 1: Prioritize Manual Testing First**
- Date: 2025-01-24
- Rationale: If CLI works manually, bug is only in test setup (lower severity)
- If CLI fails manually, this is critical and blocks soft open source

**Decision 2: Mark Test as Ignored for Now**
- Date: 2025-01-24
- Rationale: Don't block Issue 10 completion on flaky test
- But create this issue to ensure we come back to it
- Status: Current state (7 passing tests, 1 ignored)

## Future Work

If file watcher proves unreliable:
- Consider alternative: Poll-based file monitoring (less efficient, more reliable)
- Consider alternative: Manual refresh command (`noet watch --poll`)
- Consider alternative: inotify/FSEvents direct integration (platform-specific)

---

**Status**: Created 2025-01-24, not started
**Next Step**: Manual CLI testing (Step 1) to determine if real bug or test artifact