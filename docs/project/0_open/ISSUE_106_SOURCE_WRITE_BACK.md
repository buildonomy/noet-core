---
version = "0.1"
title = "Issue 106: Source Write-Back and Redline Promotion"
---

# Issue 106: Source Write-Back and Redline Promotion

**Priority**: MEDIUM
**Estimated Effort**: 3 days (RELATIVE COMPARISON ONLY)
**Dependencies**: Requires Issue 103 (node source ranges — the byte span to
replace), Issue 102 (`noet serve` — owns the idle boundary and the write lock),
Issue 107 (generalized codec write-back — the `BeliefEvent` → source edit path
this issue drives).
**Related**: Issue 104 (the annotation record field set — a redline record is
one, and is the input to promotion); Issue 105 (the store those records live
in); **Issue 17 (registers the redline `protocol_id` this issue promotes — step
2a)**; Issue 11 Open Question 7 (delegated here).

## Summary

Every write path in the system currently terminates in a store. An annotation
records that something should change, or that something *did* change and the
source no longer matches. It cannot make the change. This issue closes the loop:
a graph-level edit becomes an actual edit to the source file that produced the
node.

The interesting part is not the splice — that is `source[range] = new_content`.
It is doing the write safely while a watcher, a compiler, and possibly a second
independent writer are all touching the same file.

## Goals

1. A single writer that turns a graph-level edit into a source-file edit
2. Atomic writes: no reader ever observes a partially-written source file
3. The writer coordinates with the watcher so a self-write does not trigger a
   re-parse loop
4. Redline promotion: applying a redline record is itself an appended record
5. Stale-range detection: refuse the write rather than clobber a changed file

## Architecture

### The write path

```
graph-level edit  (annotation action, redline promotion, LSP code action)
        │
        ▼
resolve target node → source path + byte range        (Issue 103)
        │
        ▼
verify the span still hashes to what was read
        │
        ▼
splice replacement into source[range]
        │
        ▼
write temp file in the same directory → fsync → rename over the target
        │
        ▼
register the path with the watcher's ignore set before the rename lands
        │
        ▼
append a record describing the promotion                (Issue 105 store)
```

Step 2 is Issue 103's `range_of(bid)`. Steps 4–5 are the substance of this issue.

### Atomicity is the real content of this issue

The current state of the code, verified:

- **`DocumentCompiler::parse_one_path`** (`src/codec/compiler.rs`) writes
  rewritten source content directly via `tokio::fs::write` — three call sites
  (rewritten text, rewritten binary, and the byte-serialization path). No temp
  file, no rename. A reader that opens the file mid-write sees a truncated
  document.
- **`WatchService`'s `ignored_write_paths`** (`src/watch.rs`) is an
  `Arc<Mutex<HashSet<PathBuf>>>` consulted by the debouncer to suppress
  re-parse triggers for writes made by **that same `WatchService` instance**.
  It is flushed when the compiler queues drain. It provides **no atomicity**,
  and it does nothing about a second, independent writer — a writer in another
  process has no handle on that set.

Issue 11 Open Question 7 states this problem precisely and resolves it by
delegating here. **Issue 106 now owns it.** The two named second-writer cases
are exactly the ones that matter:

- **LSP `textDocument/didSave`** — the editor writes the buffer; a concurrently
  running `noet watch` in another process may be writing the same file.
- **Any browser- or CLI-initiated edit** through the viewer or a PII surface —
  same shape, different origin.

What this issue must specify:

1. **Temp-file + rename** for every source write, including the three existing
   `parse_one_path` sites. Temp file in the *same directory* as the target so
   the rename stays within one filesystem.
2. **How a write registers with the server** so the watcher ignores it. The
   `ignored_write_paths` set is instance-local; if the writer is to be shared
   between LSP, viewer, and compiler, the registration must go through the
   server (Issue 102) rather than through a `WatchService` handle. Decide and
   record which: a shared handle when in-process, or a server request when not.
3. **What happens with no server running.** `noet parse` writes with no watcher
   present. The writer must work standalone; ignore-set registration becomes a
   no-op rather than an error.

### Redline promotion

A **redline annotation record** — an annotation whose `protocol_id` marks it as
proposing a change to the node it anchors (field set: Issue 104; store: Issue
105) — describes a proposed change. It has, until now, been a claim with nowhere
to land. Promoting it means applying it to source.

Two properties must hold:

- **The promotion is itself an appended record.** Its `caused_by` cites the redline
  it promoted. The audit trail therefore survives the edit: you can ask "why
  does the source say this?" and traverse back to the redline record that
  motivated it. This follows directly from the append-only constraint — promotion
  does not mutate or consume the redline.
- **The promoted edit is a normal source change.** Once written, it is an
  ordinary diff in the working tree that flows through git — review, commit,
  PR — exactly as a hand-typed edit would. Nothing about promotion makes the
  resulting change special or bypasses review.

### Concurrency and safety

- **Writes happen only at the server's idle boundary** (Issue 102). Writing
  while a parse epoch is in flight means splicing into a file whose ranges the
  compiler is mid-way through recomputing.
- **Stale-range detection.** A byte range is valid only for the source text it
  was computed from (Issue 103's stated risk). Before splicing, hash the current
  bytes of `source[range]` and compare against the hash captured when the range
  was resolved. On mismatch: **refuse the write and surface a conflict.** Do not
  attempt a fuzzy re-anchor and do not clobber. A refused write with a clear
  conflict is recoverable; a mis-spliced document may not be noticed for weeks.
- **One writer at a time.** The server holds a per-path write lock for the
  duration of hash-check → splice → rename.

### Scope boundary

This issue delivers **the mechanism, not the UI**. The viewer's "edit" action,
the LSP code action, and any CLI promotion command are *consumers* of this
writer and are specified in their own issues. If this issue grows a user-facing
affordance, it has scope-crept.

Also out of scope: multi-node and multi-file edits as a single transaction. One
edit, one range, one file. Batching is a later concern and should not shape the
Phase 1 interface beyond keeping the entry point per-edit.

## Implementation Steps

1. **Atomic writer primitive** (0.75 days)
   - [ ] `write_atomic(path, bytes)` — temp file in the target's directory,
         fsync, rename
   - [ ] Convert the three `parse_one_path` write sites to use it
   - [ ] Standalone behaviour when no watcher/server is present

2. **Watcher coordination** (0.5 days)
   - [ ] Decide shared-handle vs server-request registration; record the choice
   - [ ] Register the target path before the rename, not after
   - [ ] Confirm the existing flush-on-idle behaviour still clears the set

3. **Span-hash guard** (0.5 days)
   - [ ] Capture a hash of `source[range]` when a range is resolved
   - [ ] Re-verify immediately before splicing under the write lock
   - [ ] Conflict error type carrying the node, path, and expected/actual hash

4. **Graph-edit → source-edit entry point** (0.75 days)
   - [ ] Takes a target BID, a replacement string, and the captured span hash
   - [ ] Gated on the Issue 102 idle boundary
   - [ ] Appends the promotion record on success

5. **Redline promotion** (0.5 days)
   - [ ] Resolve a redline record to a target range and replacement
   - [ ] Append the promotion record with `caused_by` citing the redline
   - [ ] Verify the redline itself is unmodified afterwards

## Testing Requirements

- Write-back replaces exactly the target span and leaves the rest of the file
  byte-identical
- A concurrent reader looping on the file never observes a truncated or
  partially-written document across many write iterations
- A write does not trigger a re-parse: the watcher's compile generation does not
  advance from a self-write
- A write whose span hash no longer matches is refused with a conflict error and
  leaves the file untouched
- Promoting a redline appends a record citing it; the redline record is
  unchanged on disk
- The writer works with no server running (`noet parse` path)

## Success Criteria

- [ ] No source write in the codebase uses a bare `tokio::fs::write`
- [ ] A self-write provably does not re-trigger a parse
- [ ] Stale-range writes are refused, never applied
- [ ] Redline promotion produces both a source edit and an audit record
- [ ] Issue 11 Open Question 7 is closable: both (a) and (b) from its Decision
      are answered by this issue's writer
- [ ] No user-facing edit affordance shipped here

## Risks

- **Stale ranges cause mis-splices.** A range resolved before an external edit
  points at the wrong bytes. → **Mitigation**: span content hash checked under
  the write lock immediately before the splice; refuse on mismatch.
- **Write loops with the watcher.** The writer's own write is observed as an
  external change, triggers a parse, which rewrites, which triggers a parse.
  → **Mitigation**: register with the ignore set before the rename, and gate
  writes on the idle boundary so a loop cannot compound.
- **Partial writes visible to a concurrent reader.** → **Mitigation**: rename is
  atomic on POSIX. **Windows caveat**: `ReplaceFile`/`MoveFileEx` semantics
  differ and a rename over an open file can fail; the writer needs a
  platform-specific path there, and it must be tested rather than assumed.
- **Idle-boundary starvation.** On a corpus that is never idle, writes never
  land. → **Mitigation**: the boundary is Issue 102's to define; this issue
  should surface a timeout rather than block indefinitely.
- **Temp files left behind on crash.** → **Mitigation**: a recognizable temp
  name pattern in the target directory, cleaned on next write to that path.

## Open Questions

- **Shared handle or server request** for ignore-set registration? Depends on
  whether the LSP is expected to run in-process with the watcher (Issue 11
  Decision 2 leans "share instance") or as a separate process. Needs a decision
  before Step 2.
- **What is the promotion record's `protocol_id`?** Promotion is a distinct act
  from the redline it promotes, so it probably wants its own registry entry
  rather than reusing the redline's. **The redline entry itself is now Issue 17
  step 2a's** (it registers the procedural annotation subtypes, redline being the
  worked example). Whether the *promotion* entry belongs there too, or with
  Issue 104's general vocabulary, is still open.
- **Section range semantics** — resolved by Issue 103: it stores the heading
  range, and the subtree extent is derived in O(1) from child ordering, so this
  issue computes the subtree span it needs rather than reading a stored one.
- **Should a refused write offer a re-anchor?** Recommend no for Phase 1 —
  surface the conflict and let the caller re-resolve the range against fresh
  content.

## References

- `src/codec/compiler.rs` — `parse_one_path`, the three bare `tokio::fs::write`
  source-write sites
- `src/watch.rs` — `FileUpdateSyncer.ignored_write_paths`, debouncer suppression,
  flush-on-idle
- Issue 103 — node source ranges, `range_of(bid)`, the stale-range risk
- Issue 102 — `noet serve`, idle boundary, write lock
- Issue 104 — the annotation record field set and `protocol_id`; a redline record
  is the input this promotes
- Issue 105 — the store redline records are read from and promotion records are
  appended to
- Issue 11 Open Question 7 — the statement of the atomicity problem, delegated here
- `docs/design/core/beliefbase_architecture.md` §4.3 — `caused_by` and append-only
  record semantics
