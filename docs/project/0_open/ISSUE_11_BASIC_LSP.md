# Issue 11: Basic Language Server Protocol (LSP) Implementation

> [!NOTE]
> **Re-scoped — this issue is now LSP protocol work only.** Four changes:
>
> 1. **The LSP is a PII (Personal Inspection Interface) surface**, not a
>    compilation feature — see `docs/design/annotation/attestation_fabric.md` §13.
>    Editor diagnostics are inference-engine gap findings (Issue 93); code
>    actions are procedures firing from rule maps; hover is metadata card
>    content rendered inline. The feature set is driven by the inference
>    engine's output, not by the compiler's diagnostics alone.
> 2. **Position tracking has been split out to Issue 103** (node source
>    ranges). This issue no longer designs or builds it; it consumes it.
> 3. **The LSP attaches to `noet serve` (Issue 102)** for the shared live
>    graph. The `DaemonService` model this issue was originally written
>    against no longer exists — `src/daemon.rs` is gone, and Issue 10 is
>    completed and superseded by the serve model.
> 4. **Diagnostics come from `DocumentCompiler::last_diagnostics()`**
>    (Issue 66), not from a daemon-owned diagnostic cache.

**Priority**: HIGH - Enables IDE integration for v0.2.0
**Estimated Effort**: 2-3 days
**Dependencies**: Requires Issue 102 (`noet serve` — shared live graph and
consumer registry), Issue 103 (node source ranges), Issue 66
(`DocumentCompiler::last_diagnostics`). Informed by the attestation service
(`attestation_fabric.md` §13) and Issue 93 (inference engine).
**Target Version**: v0.2.0 (post-open source, pre-announcement)
**Context**: LSP integration positions noet as a "real" language with modern tooling support

## Summary

Build an **LSP shim**: a translation layer between the annotation model and the
Language Server Protocol. Editors get real-time diagnostics and hover in VSCode,
Zed, Neovim, and any other LSP client; noet gets one more PII surface
(`attestation_fabric.md` §13) rather than a bespoke subsystem.

The shim is deliberately thin. It does two things:

| Direction | Translation |
|---|---|
| **Annotation → LSP** | records anchored to a node render as `publishDiagnostics`, hover content, and code lenses |
| **LSP → Annotation** | editor actions — resolving a diagnostic, closing a todo, signing off — emit records |

Everything else already exists: Issue 103 supplies node source ranges, Issue 102
holds the live graph and the consumer registry, Issue 105 stores the records.
The shim maps between `(bid, range)` and LSP's `(uri, position)`, and between
record kinds and LSP concepts. `tower-lsp` handles the protocol.

### Diagnostics are annotations

A compiler diagnostic — an unresolved reference, a collision warning — is an
observation about a node made by an identified `P` (the compiler), anchored to a
version. That is an annotation (`living_corpus.md` §2), and treating it as one
unifies three things that would otherwise need separate plumbing:

- A compiler diagnostic and a human `{todo}` on the same line are the same kind
  of object, rendered the same way, dismissed the same way.
- The inference engine's gap findings (Issue 93) reach the editor with no new
  mechanism — they are annotations from a different `P`.
- "Resolve this diagnostic" and "close this todo" are one code action.

**But compiler diagnostics are *ephemeral* annotations, and the distinction is
load-bearing.** They are derived from the current parse, regenerated on every
compile, and must not accumulate in the sidecar store — which is sized for
deliberate human acts and is append-only. Persisting a diagnostic that a reparse
would re-derive is the volume failure `living_corpus.md` §2 warns about, in a new
form.

So annotations divide by lifetime, not only by kind:

| | Durable | Ephemeral |
|---|---|---|
| Examples | `{todo}`, `{reviewed}`, redlines | compiler diagnostics, inference findings, cursors, presence |
| Produced by | a human act | recomputation |
| Stored | Issue 105 sidecar | nowhere — held in the live session |
| Survives restart | yes | no, and should not |
| Routed by | Issue 102's registry | Issue 102's registry |

Both travel the same path and render through the same surface. Only the durable
ones are written. **`living_corpus.md` does not yet name this split** — see Open
Questions; it is a design-doc gap this issue surfaces rather than one it should
resolve unilaterally.

> Cursors and presence are the other ephemeral case and belong to the same
> mechanism: an actor's position is an observation about a node that is never
> written down. Out of scope here, noted so the split is designed once.

**User Experience**: Users edit markdown documents in their IDE, see parse errors as they type, hover over headings/links to see metadata (BID, node type, resolved references), and get immediate feedback on broken references.

**Post-Implementation**: noet documents have the same IDE experience as code (diagnostics, hover, etc.), significantly lowering the barrier to adoption — and the editor becomes a write surface for annotations, not only a read surface for diagnostics.

## Goals

1. **Translate annotations to LSP** — records anchored to a node become
   `publishDiagnostics` and hover content; one path, whether the record came
   from the compiler, a human, or the inference engine
2. **Translate LSP actions to annotations** — an editor action emits a record
   through Issue 102, exactly as the viewer does
3. **Distinguish ephemeral from durable** — compiler diagnostics are recomputed
   and must not reach the Issue 105 sidecar
4. Consume Issue 103's node source ranges to map `(bid, range)` ↔ `(uri, position)`
5. Implement the LSP server using `tower-lsp` with JSON-RPC over stdio
6. Provide document synchronization (didOpen, didChange, didSave, didClose),
   full-document sync mode (incremental deferred to Issue 12)
7. Create VSCode extension configuration for testing
8. Document IDE setup for VSCode, Zed, Neovim

## Architecture

### LSP Components

```
┌─────────────────────────────────────────────────┐
│  IDE (VSCode, Zed, Neovim, etc.)                │
│  - User edits document in memory                │
│  - Sends LSP requests over stdio                │
│  - Displays diagnostics inline                  │
│  - Shows hover information                      │
└────────────────┬────────────────────────────────┘
                 │ JSON-RPC 2.0 over stdio
                 │
┌────────────────▼────────────────────────────────┐
│  noet lsp                                       │
│  - Implements tower_lsp::LanguageServer         │
│  - Manages in-memory document state             │
│  - Converts: LSP types ↔ noet types             │
│  - Registers as a consumer of `noet serve`      │
└────────────────┬────────────────────────────────┘
                 │ Consumer registry (Issue 102)
                 │
┌────────────────▼────────────────────────────────┐
│  noet serve (Issue 102)                         │
│  - Owns the authoritative in-memory graph       │
│  - Parses in-memory documents incrementally     │
│  - Exposes DocumentCompiler::last_diagnostics() │
│  - Resolves cross-document references           │
│  - Node source ranges from Issue 103            │
└─────────────────────────────────────────────────┘
```

The LSP is **one consumer among several**. `noet serve` also serves the browser
viewer, the MCP server, and the annotation client (Issue 105) from the same live
graph, and owns the idle boundary at which the graph is safe to read. The LSP has
no private compiler and no private cache; it holds only editor-side document text
and the LSP-shaped projection of what `serve` already knows.

### Data Structures

**Position Tracking**: specified in
[`ISSUE_103_LIVING_CORPUS_INFRASTRUCTURE.md`](./ISSUE_103_LIVING_CORPUS_INFRASTRUCTURE.md) and not
duplicated here. Issue 103 delivers byte-offset source ranges per node, a
position → BID lookup for a single document, and ranges on diagnostics. The LSP
converts byte offsets to LSP `Position` values at the protocol boundary using the
existing `codec::byte_offset_to_location`. The `PositionIndex` / `IntervalTree`
design previously sketched in this section was superseded during that split —
Issue 103 uses a sorted `Vec` with binary search instead.

**LSP Server State**:
```rust
struct NoetLanguageServer {
    client: Client,                              // LSP client connection
    graph: ServeHandle,                          // Live graph handle from Issue 102
    documents: Arc<RwLock<HashMap<Url, String>>>, // In-memory document state
    diagnostics: Arc<RwLock<HashMap<Url, Vec<Diagnostic>>>>, // Cached diagnostics
}
```

### LSP Capabilities (Phase 1 - This Issue)

**Implemented**:
- ✅ `initialize` / `initialized` - server lifecycle
- ✅ `shutdown` / `exit` - graceful termination
- ✅ `textDocument/didOpen` - document opened in editor
- ✅ `textDocument/didChange` - document modified (full sync)
- ✅ `textDocument/didSave` - document saved to disk
- ✅ `textDocument/didClose` - document closed
- ✅ `textDocument/publishDiagnostics` - send errors/warnings to editor
- ✅ `textDocument/hover` - show node metadata on hover

**Deferred to Issue 12**:
- ⏭️ `textDocument/definition` - go to definition
- ⏭️ `textDocument/references` - find all references
- ⏭️ `textDocument/documentSymbol` - document outline
- ⏭️ `textDocument/completion` - autocomplete references
- ⏭️ `textDocument/formatting` - format document, inject BIDs
- ⏭️ `textDocument/codeAction` - quick fixes

## Implementation Steps

### 1. Consume Issue 103's node ranges (0 days — delivered by Issue 103)

Position tracking is specified and built in [`ISSUE_103_LIVING_CORPUS_INFRASTRUCTURE.md`](./ISSUE_103_LIVING_CORPUS_INFRASTRUCTURE.md). This step is a consumption point only: convert Issue 103's byte-offset ranges to LSP `Position` values via `codec::byte_offset_to_location` at the protocol boundary.

### 2. Implement LSP Server with tower-lsp (1-2 days)

**Objective**: Create LSP server binary with basic protocol support

**New file**: `src/bin/noet-lsp.rs`
```rust
use tower_lsp::{LspService, Server};

#[tokio::main]
async fn main() {
    let stdin = tokio::io::stdin();
    let stdout = tokio::io::stdout();

    let (service, socket) = LspService::new(|client| {
        NoetLanguageServer::new(client)
    });

    Server::new(stdin, stdout, socket).serve(service).await;
}
```

**Implementation tasks**:
- [ ] Add `tower-lsp` and `lsp-types` dependencies to `Cargo.toml`
- [ ] Create `NoetLanguageServer` struct implementing `LanguageServer` trait
- [ ] Implement `initialize` - declare server capabilities
- [ ] Implement `initialized` - server ready notification
- [ ] Implement `shutdown` and `exit` - graceful termination
- [ ] Set up tracing/logging for LSP server
- [ ] Handle protocol errors gracefully

**Testing**:
- [ ] Test: Server starts and responds to initialize
- [ ] Test: Server handles shutdown gracefully
- [ ] Test: Invalid messages don't crash server

### 3. Implement Document Synchronization (1 day)

**Objective**: Keep editor state synchronized with parser

**Document lifecycle**:
- [ ] Implement `textDocument/didOpen`:
  - Store document content in memory
  - Parse document content
  - Generate and publish diagnostics
- [ ] Implement `textDocument/didChange`:
  - Update in-memory content (full sync mode)
  - Re-parse changed document
  - Update diagnostics
- [ ] Implement `textDocument/didSave`:
  - Optional: Write to filesystem if requested
  - Trigger dependent document re-parsing
- [ ] Implement `textDocument/didClose`:
  - Remove from in-memory cache
  - Clean up diagnostics

**Coordination with `noet serve`**:
- [ ] Attach to the Issue 102 consumer registry rather than owning a compiler instance
- [ ] Handle conflicts between editor changes and filesystem changes
- [ ] Prioritize editor state over filesystem when document is open
- [ ] Document synchronization semantics

**Testing**:
- [ ] Test: Open document, verify diagnostics published
- [ ] Test: Edit document, verify diagnostics update
- [ ] Test: Close document, verify state cleaned up
- [ ] Test: Multiple documents open simultaneously

### 4. Implement Diagnostics Publishing (0.5 days)

**Objective**: Send parse errors/warnings to editor

**Implementation**:
- [ ] Convert `ParseDiagnostic` → `lsp_types::Diagnostic`
- [ ] Map diagnostic severity: Error, Warning, Info
- [ ] Include diagnostic ranges in published messages
- [ ] Handle multiple diagnostics per document
- [ ] Clear diagnostics when document is valid

**Diagnostic types to publish**:
- [ ] `ParseError` → LSP Error
- [ ] `UnresolvedReference` → LSP Warning
- [ ] `Warning` → LSP Warning
- [ ] `Info` → LSP Information

**Testing**:
- [ ] Test: Parse error appears in editor
- [ ] Test: Unresolved reference shows as warning
- [ ] Test: Diagnostics clear when fixed
- [ ] Test: Multiple diagnostics in one document

### 5. Implement Hover Provider (0.5 days)

**Objective**: Show node metadata when hovering over headings/links

**Implementation**:
- [ ] Implement `textDocument/hover` method
- [ ] Get node at cursor position using `get_node_at_position`
- [ ] Format node metadata as Markdown hover content:
  ```markdown
  **Node Title**
  
  BID: `12345678-1234-...`
  Kind: Document
  Schema: action
  
  ---
  [Go to definition](#)  <!-- Future: Issue 12 -->
  ```
- [ ] Handle hover over links (show target node info)
- [ ] Handle hover over BIDs (show node info)
- [ ] Handle hover in empty space (no hover)

**Testing**:
- [ ] Test: Hover over heading shows node info
- [ ] Test: Hover over link shows target info
- [ ] Test: Hover over BID shows node info
- [ ] Test: Hover in empty space returns None

### 6. Create Editor Configuration Files (0.5 days)

**Objective**: Make it easy to test LSP in different editors

**VSCode extension configuration**:
- [ ] Create `.vscode/extensions/noet/package.json`:
  ```json
  {
    "name": "noet",
    "displayName": "noet Language Support",
    "description": "Language server for noet documents",
    "version": "0.1.0",
    "engines": { "vscode": "^1.75.0" },
    "activationEvents": ["onLanguage:markdown"],
    "main": "./out/extension.js",
    "contributes": {
      "languages": [{
        "id": "noet",
        "extensions": [".md", ".toml"]
      }],
      "configuration": {
        "title": "noet",
        "properties": {
          "noet.lsp.path": {
            "type": "string",
            "default": "noet-lsp",
            "description": "Path to noet LSP server"
          }
        }
      }
    }
  }
  ```
- [ ] Create basic TypeScript extension code
- [ ] Document VSCode setup in README

**Zed configuration**:
- [ ] Create `.zed/languages/noet.json` config
- [ ] Document Zed setup

**Neovim configuration**:
- [ ] Create example `init.lua` snippet using `lspconfig`
- [ ] Document Neovim setup

**Testing**:
- [ ] Test LSP works in VSCode
- [ ] Test LSP works in Zed
- [ ] Test LSP works in Neovim (if possible)

### 7. Audit and Convert Tracing Logs to Diagnostics (1-2 days)
  
**Goal**: Convert parse-time tracing logs to structured ParseDiagnostic for LSP integration.
  
**Pattern**: Use instrumentation design pattern from `docs/design/presentation/instrumentation_design.md`
  
**Tasks**:
1. Audit codebase for `tracing::{warn, info, debug}` calls related to parsing:
   - `src/codec/builder.rs` - BID mismatches, cache_fetch issues
   - `src/codec/md.rs` - ID normalization, collision detection
   - `src/paths.rs` - Title collisions in title_map
   - `src/codec/belief_ir.rs` - TOML parsing errors
  
2. Create `DiagnosticCaptureLayer` following instrumentation pattern:
   ```rust
   struct DiagnosticCaptureLayer {
       state: Arc<Mutex<DiagnosticState>>,
   }
   ```
  
3. Add `capture_diagnostic!` macro (similar to `capture_data!`):
   ```rust
   capture_diagnostic!(ParseDiagnostic::warning("...")
       .with_location(line, column));
   ```
  
4. Dual-emit during migration:
   - Keep existing tracing for debugging
   - Add structured diagnostic capture
   - Remove tracing once LSP integration working
  
5. Modify DocCodec trait to return diagnostics:
   ```rust
   fn parse(...) -> Result<(Vec<IRNode>, Vec<ParseDiagnostic>)>
   ```
  
**Integration Points**:
- Use RoutingLayer pattern to separate diagnostic capture from logging
- Zero-cost when disabled (crucial for performance)
- Thread-safe via Arc<Mutex> (proven in instrumentation design)
  
**Examples from Issue 22**:
- Title collision warnings
- Explicit ID collision warnings  
- BID mismatch diagnostics
- Network-level ID collision warnings
  
**Related**:
- `docs/design/presentation/instrumentation_design.md` - Proven pattern
- Issue 22 - Uses tracing now, marked with TODO for conversion
  
### 8. Documentation and Examples (0.5 days)

**Objective**: Enable users to set up and use LSP

**Documentation to create**:
- [ ] Add "IDE Integration" section to main README
- [ ] Create `docs/lsp.md` - detailed LSP documentation:
  - Supported features
  - Editor setup instructions (VSCode, Zed, Neovim)
  - Troubleshooting guide
  - Architecture overview
- [ ] Add doctests to `bin/noet-lsp.rs` showing usage
- [ ] Update `lib.rs` rustdoc to mention LSP support

**Update Issue 5 documentation**:
- [ ] Add LSP section to architecture docs
- [ ] Document position tracking in parser docs
- [ ] Link IDE integration from main docs

**Testing**:
- [ ] Documentation review for clarity
- [ ] Verify setup instructions work on clean system
- [ ] Test troubleshooting steps resolve common issues

## Testing Requirements
 
### Diagnostic Capture Testing
- Test `DiagnosticCaptureLayer` captures events correctly
- Test diagnostics include accurate location information
- Test zero-cost when disabled (benchmark parsing)
- Test thread-safety under concurrent parse operations

### Unit Tests
- Position tracking in parser returns correct ranges
- `get_node_at_position` returns correct node
- Diagnostic conversion to LSP types works correctly
- Hover content formatting produces valid Markdown

### Integration Tests
- LSP server starts and initializes successfully
- Document open/change/close cycle works correctly
- Diagnostics appear in editor after parse errors
- Hover shows correct information
- Multiple documents can be open simultaneously
- Server shutdown is graceful

### Manual Testing in IDEs
- VSCode: Open document, see diagnostics, hover works
- Zed: Open document, see diagnostics, hover works
- Neovim: Open document, see diagnostics, hover works
- Test with real noet documents (examples from basic_usage)
- Test with documents containing errors
- Test with cross-document references

### Performance Testing
- LSP responds to changes within 100ms for small documents (<1MB)
- No memory leaks during long editing sessions
- Handles 10+ open documents without degradation

## Success Criteria

- [ ] LSP server binary (`noet-lsp`) compiles and runs
- [ ] Server implements basic LSP lifecycle (initialize, shutdown, exit)
- [ ] Document synchronization works (didOpen, didChange, didSave, didClose)
- [ ] Diagnostics appear in editor in real-time
- [ ] Hover shows node metadata (BID, kind, schema)
- [ ] Parser tracks positions for all nodes and diagnostics
- [ ] Tested working in at least 2 IDEs (VSCode + one other)
- [ ] Documentation enables users to set up LSP
- [ ] No blocking issues for Issue 12 (advanced LSP features)

## Risks

**Risk**: Position tracking breaks existing parser functionality  
**Mitigation**: Add position tracking as optional feature first; extensive testing; keep ranges in separate struct if needed

**Risk**: LSP protocol complexity causes delays  
**Mitigation**: Use `tower-lsp` to handle protocol details; start with minimal feature set; defer complex features to Issue 12

**Risk**: Performance issues with large documents  
**Mitigation**: Profile parser with position tracking; optimize hot paths; consider incremental parsing in Issue 12

**Risk**: IDE-specific compatibility issues  
**Mitigation**: Test in multiple editors early; follow LSP spec strictly; document known limitations

**Risk**: Synchronization conflicts between editor and filesystem  
**Mitigation**: Editor state always wins when document is open; document behavior clearly; test conflict scenarios

**Risk**: tower-lsp API changes or limitations  
**Mitigation**: Pin to stable version; read tower-lsp source code; have fallback plan to use lsp-server if needed

## Open Questions

0. **The ephemeral/durable annotation split is not in the design docs.**
   `living_corpus.md` §2 distinguishes annotations from general `R` by subject
   and volume, but assumes all annotations are written to the Issue 105 store.
   Compiler diagnostics, inference findings, cursors, and presence are
   annotations by that definition and must **not** be stored — they are derived
   or transient, and persisting them defeats the volume argument that keeps the
   store human-scale.

   **The likely shape is not a new class but an existing one extended.** Issue
   105 already defines a scope hierarchy — repo / user / shared — with union
   read semantics and precedence governing writes. An **in-memory scope** sits
   below repo as the most local: records live there, are readable and
   projectable exactly like any other, and simply never persist. A diagnostic
   does not "skip the sidecar"; it lives in the most-local sidecar and never
   extends past it.

   That reframing makes the governing mechanism a **protocol property**: what a
   record kind does when its anchor version goes stale, and what it does on a
   flush. Roughly:

   | Kind | On stale version | On flush |
   |---|---|---|
   | compiler diagnostic | discard — recomputed | never promotes |
   | cursor / presence | discard | never promotes |
   | `{todo}` | mark stale, keep | promote to repo scope |
   | draft / working record | keep | promote when the run closes |

   Three things follow, none of which belongs to this issue alone:

   - **Staleness policy is per-`record_kind`**, declared alongside the anchor
     scope the kind already selects (`content_versioning.md` §3). Discard,
     retain-and-mark, and re-derive are the plausible values.
   - **Flush is promotion between scopes**, which is also how percolation works
     at a federation boundary (`federated_belief_network.md` §1.2 — a child run
     promotes its `RunEnd` summary to the shared queue and keeps its working
     records local). Same mechanism, different boundary.
   - **Issue 102 routes all kinds identically**; the scope decides persistence,
     not the router.

   Still open: whether an ephemeral annotation projects into the compiled graph
   or renders only at the surface. Projecting a diagnostic as a node puts derived
   data in the graph, which §4's assert/mutate boundary argues against.

   **Recommend**: `living_corpus.md` §2 gains the in-memory scope, Issue 105
   extends its hierarchy downward, Issue 105 owns flush semantics (it already
   owns run brackets, and a flush is a close). Raise before implementing step 4.

1. **Incremental document sync in Issue 11 or defer to Issue 12?**
   - **Decision**: Defer to Issue 12. Use full-document sync (TextDocumentSyncKind::FULL) for simplicity. Most editors handle this fine for markdown documents.

2. **Should LSP server share the compiler instance or create its own?**
   - **Decision**: Share. The shared instance is now `noet serve`'s live graph (Issue 102), not a `DaemonService`. Allows coordination between filesystem changes and editor changes. Document synchronization semantics carefully.

3. **How to handle documents with no BIDs yet?**
   - **Decision**: LSP works fine without BIDs. Hover shows "No BID yet" and suggests running `noet parse` to inject BIDs.

4. **Support for both .md and .toml files in LSP?**
   - **Decision**: Yes. Register LSP for both file types. Document how to configure editor to use noet for both.

5. **How to distribute VSCode extension?**
   - **Decision**: For v0.2.0, document manual installation. For v0.3.0+, consider publishing to VSCode marketplace.

6. **Where do per-node diagnostics and source positions live?**

   > **Resolved — moved to Issue 103.** The design below is carried forward in
   > [`ISSUE_103_LIVING_CORPUS_INFRASTRUCTURE.md`](./ISSUE_103_LIVING_CORPUS_INFRASTRUCTURE.md), which
   > also corrects two details that have since gone stale: `BeliefBase.diagnostics`
   > is already `Vec<ParseDiagnostic>` (not `Vec<String>`), and `BeliefNode` already
   > has a `metadata` field — but a serialized, equality-participating one, not the
   > ephemeral field proposed here. Text kept for provenance.

   LSP requires source ranges for diagnostics and hover. Currently `ParseDiagnostic`
   lives in `codec::diagnostic` and is produced in `codec::builder::push()`. Several
   problems arise when the LSP needs this information:

   - `BeliefBase` (lower layer) cannot hold `ParseDiagnostic` values without a circular
     dependency on `codec`.
   - Source position (byte offset, line/column) is available in the codec layer but is
     discarded before reaching `BeliefBase` or `global_bb`.
   - Collision diagnostics detected in `BeliefBase::insert_state` (inter-epoch path)
     currently have no access to `ParseDiagnostic` — they write to a stepping-stone
     `BeliefBase.diagnostics: SharedLock<Vec<String>>` and are drained by `push()`.

   **Proposed design — `BeliefNode` metadata field**:

   Add a `metadata: HashMap<String, MetadataValue>` field to `BeliefNode` in
   `src/properties.rs`. This field:
   - Is **excluded from `toml()` serialization** and from `PartialEq` / `Hash` comparisons
     (i.e. `node.toml()` is unchanged; equality is purely semantic content).
   - Carries ephemeral per-node context: source filepath, byte range, parse diagnostics.
   - Is populated by the codec layer after `push()` resolves the node's BID.
   - Is accessible to `BeliefBase::insert_state` without any layering change — `metadata`
     is on the node struct itself, not on `BeliefBase`.
   - Gives the LSP server direct access to `PositionIndex`-style data without a separate
     index, since every `BeliefNode` in `doc_bb` carries its own range.

   This eliminates the need for the `BeliefBase.diagnostics` stepping-stone field once
   implemented. The stepping-stone remains until this design is built.

   **Tradeoffs**:
   - Adds a field to `BeliefNode` that is silent in serialization — must be carefully
     documented so future `toml()` callers don't expect it.
   - `BeliefGraph` merge/union operations must decide how to merge `metadata` (last-write
     wins, or union of diagnostics). Semantics need specifying.
   - `Clone` of `BeliefNode` clones metadata too — ephemeral data survives longer than
     intended in caches. Must be cleared on cache eviction.

   **Decision**: Deferred to implementation of Issue 11 Step 1 (position tracking in
   DocCodec). Stepping-stone `BeliefBase.diagnostics: SharedLock<Vec<String>>` remains
   until then. See `src/codec/builder.rs` `push()` and `src/beliefbase/base.rs`
   `insert_state` for current collision diagnostic wiring.

7. **Are atomic (temp-file + rename) writes needed for LSP-initiated writes?**

   > **Resolved — delegated to Issue 106.** Source write-back and redline promotion
   > (Issue 106) owns the graph-edit → atomic source-file-write path. The LSP is one
   > caller of that path, not an independent solver of it: `textDocument/didSave`
   > and BID-injection-on-format go through Issue 106's writer, which owns both the
   > temp-file+rename atomicity and the coordination with the watcher. The analysis
   > below stands as the statement of the problem Issue 106 must solve.

   `DocumentCompiler::parse_one_path` writes directly via `tokio::fs::write` — no
   temp file, no rename (`src/codec/compiler.rs`). The watch service's
   `ignored_write_paths` set (`src/watch.rs`) only suppresses re-parse triggers for
   writes made by that *same* `WatchService` instance; it provides no atomicity and
   doesn't help against a second, independent writer. `textDocument/didSave` (§3
   above) and BID-injection-on-format (Issue 12 §2.2) are exactly that: a second
   writer to files a concurrently-running `noet watch` may also be touching. A
   non-atomic write leaves a window where a concurrent watcher could observe a
   partially-written file.
   - **Decision**: TBD — either (a) share the watch instance's `ignored_write_paths`
     with the LSP writer (consistent with Decision 2 above, "share instance"), or
     (b) add temp-file+rename atomicity to the write path if LSP and watch are
     expected to run as separate processes/instances.

## Future Work (Issue 12)

**Navigation features** (2-3 days):
- `textDocument/definition` - go to definition on `[[links]]`
- `textDocument/references` - find all references to node
- `textDocument/documentSymbol` - document outline in sidebar
- `textDocument/documentLink` - make `[[links]]` clickable
- `workspace/symbol` - search symbols across workspace

**Editing features** (3-4 days):
- `textDocument/completion` - autocomplete `[[references]]`
- `textDocument/formatting` - inject BIDs, format links
- `textDocument/codeAction` - quick fixes for unresolved references
- `textDocument/rename` - update all references when renaming
- Incremental document sync (TextDocumentSyncKind::INCREMENTAL)

**Total effort for Issue 12**: 5-7 days

## Decision Log

**Decision 1: Use tower-lsp instead of lsp-server**
- Date: [To be filled during implementation]
- Rationale: tower-lsp provides higher-level async/await API that integrates with tokio. Reduces boilerplate and error-prone protocol handling.
- Alternative: lsp-server (lower-level, more manual work)

**Decision 2: Full document sync for Issue 11**
- Date: [To be filled during implementation]
- Rationale: Simpler to implement, sufficient for markdown documents. Incremental sync is optimization for Issue 12.
- Impact: Re-parse entire document on every change (acceptable for markdown)

**Decision 3: Position tracking via BeliefNode metadata field (deferred)**
- **Resolved — moved to Issue 103.** See
  [`ISSUE_103_LIVING_CORPUS_INFRASTRUCTURE.md`](./ISSUE_103_LIVING_CORPUS_INFRASTRUCTURE.md) Decision 2,
  which carries this design forward and reconciles it with the `metadata: Table`
  field that now exists on `BeliefNode`.
- Date: [To be filled during implementation]
- Rationale: Positions are integral to node identity in LSP context. A `metadata:
  HashMap<String, MetadataValue>` field excluded from serialization and equality gives
  the LSP layer direct access without a separate index structure and without introducing
  a `codec` → `beliefbase` circular dependency.
- Alternative considered: `BeliefBase.diagnostics: SharedLock<Vec<String>>` — implemented
  as a stepping stone (see `src/beliefbase/base.rs`). Works for current collision
  diagnostic forwarding but loses type structure and source position.
- Alternative considered: `tracing` side-channel (see `docs/design/presentation/instrumentation_design.md`)
  — adds indirection without benefit; the LSP needs a pull model, not a log stream.
- Alternative considered: Separate `PositionIndex` mapping BID → Range — more complex,
  requires a parallel data structure kept in sync with `BeliefBase` states.
- See Open Question 6 for full design rationale.

**Decision 4: LSP as separate binary (noet-lsp)**
- Date: [To be filled during implementation]
- Rationale: IDE spawns LSP server, easier to debug as separate process. Could be `noet lsp` subcommand instead.
- Decision: Start as subcommand (`noet lsp`), easy to split later if needed

## References
 
- **Instrumentation Pattern**: `docs/design/presentation/instrumentation_design.md` - Proven tracing-based capture system

- **Depends On**: Issue 102 (`noet serve`) - shared live graph, consumer registry,
  idle boundary. Supersedes the `DaemonService` model; Issue 10
  ([`2_completed/ISSUE_10_DAEMON_TESTING.md`](../2_completed/ISSUE_10_DAEMON_TESTING.md))
  is completed and its daemon framing no longer applies
- **Depends On**: [`ISSUE_103_LIVING_CORPUS_INFRASTRUCTURE.md`](./ISSUE_103_LIVING_CORPUS_INFRASTRUCTURE.md) - node source ranges and position → BID lookup for hover and diagnostic placement
- **Depends On**: [`ISSUE_66_INCREMENTAL_PARSE.md`](./ISSUE_66_INCREMENTAL_PARSE.md) - `last_diagnostics` accessor on `DocumentCompiler` required for `publishDiagnostics`
- **Uses**: Issue 106 (source write-back) - atomic write path for LSP-initiated writes
- **Enables**: [`ISSUE_12_ADVANCED_LSP.md`](./ISSUE_12_ADVANCED_LSP.md) - advanced LSP features
- **Roadmap**: To be added to v0.2.0 section of roadmap
- **LSP Specification**: https://microsoft.github.io/language-server-protocol/
- **tower-lsp**: https://github.com/ebkalderon/tower-lsp
- **lsp-types**: https://docs.rs/lsp-types/
- **Examples**:
  - rust-analyzer: https://github.com/rust-lang/rust-analyzer
  - marksman (markdown LSP): https://github.com/artempyanykh/marksman
  - zeta-note (zettelkasten LSP): https://github.com/artempyanykh/zeta-note
- **Code Changes**:
  - `src/bin/noet-lsp.rs` - new LSP server binary (or a `noet lsp` subcommand, see Decision 4)
  - `Cargo.toml` - add tower-lsp, lsp-types dependencies
  - Note: `src/codec/builder.rs`, `src/codec/diagnostic.rs`, and `src/properties.rs`
    changes for position tracking are Issue 103's, not this issue's
- **New Files**:
  - `docs/lsp.md` - LSP documentation
  - `.vscode/extensions/noet/` - VSCode extension
  - Examples of editor configurations (Zed, Neovim)
