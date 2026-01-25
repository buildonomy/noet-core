# Issue 11: Basic Language Server Protocol (LSP) Implementation

**Priority**: HIGH - Enables IDE integration for v0.2.0
**Estimated Effort**: 3-5 days
**Dependencies**: Issue 10 (Daemon must be tested and working)
**Target Version**: v0.2.0 (post-open source, pre-announcement)
**Context**: LSP integration positions noet as a "real" language with modern tooling support

## Summary

Implement basic Language Server Protocol (LSP) support for noet, enabling IDE integration with real-time diagnostics and hover information. This transforms noet from a CLI tool into a language with first-class editor support in VSCode, Zed, Neovim, and other LSP-compatible editors. The implementation adds position tracking to the compiler, implements the LSP protocol using `tower-lsp`, and provides document synchronization between editor state and the daemon's compiler cache.

**User Experience**: Users edit markdown documents in their IDE, see parse errors as they type, hover over headings/links to see metadata (BID, node type, resolved references), and get immediate feedback on broken references.

**Post-Implementation**: noet documents have the same IDE experience as code (diagnostics, hover, etc.), significantly lowering the barrier to adoption.

## Goals

1. Add position and range tracking to the DocCodec trait (proper extension point for all parsers)
2. Implement LSP server using `tower-lsp` with JSON-RPC over stdio
3. Provide document synchronization (didOpen, didChange, didSave, didClose)
4. Publish diagnostics in real-time as documents change
5. Implement hover provider showing node metadata
6. Support full-document sync mode (incremental sync deferred to Issue 12)
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
│  noet lsp (bin/noet-lsp.rs)                     │
│  - Implements tower_lsp::LanguageServer         │
│  - Manages in-memory document state             │
│  - Converts: LSP types ↔ noet types             │
│  - Coordinates with DaemonService               │
└────────────────┬────────────────────────────────┘
                 │ Internal API
                 │
┌────────────────▼────────────────────────────────┐
│  DaemonService (src/daemon.rs)                  │
│  - Parses in-memory documents                   │
│  - Maintains BeliefBase cache                   │
│  - Generates diagnostics with ranges            │
│  - Resolves cross-document references           │
└─────────────────────────────────────────────────┘
```

### Data Structures

**Position Tracking Architecture**:

Position information is kept separate from the domain model (BeliefNode) to avoid polluting core types with presentation concerns. Instead, positions are tracked at the codec and builder layers:

```rust
use lsp_types::{Position, Range};

// NEW: Position index maintained by GraphBuilder during parse_content
pub struct PositionIndex {
    // Maps BID to its source range in the document
    node_ranges: HashMap<Bid, Range>,
    // Maps source positions to BIDs for reverse lookup
    position_tree: IntervalTree<Range, Bid>,
}

impl PositionIndex {
    pub fn get_range(&self, bid: &Bid) -> Option<Range>;
    pub fn get_node_at_position(&self, line: u32, col: u32) -> Option<Bid>;
}

pub struct ParseDiagnostic {
    pub message: String,
    pub range: Range,               // NEW: diagnostic location
    pub severity: DiagnosticSeverity,
    // ... existing fields
}

// NEW: Track link positions for navigation (codec-specific)
pub struct LinkPosition {
    pub range: Range,
    pub target: NodeKey,
    pub resolved: bool,
}
```

**Note**: BeliefNode remains unchanged - it's a domain model and shouldn't contain presentation-layer Range data.

**LSP Server State**:
```rust
struct NoetLanguageServer {
    client: Client,                              // LSP client connection
    daemon: Arc<RwLock<DaemonService>>,         // Shared daemon instance
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

### 1. Add Position Tracking to DocCodec Trait (1-2 days)

**Objective**: Extend the DocCodec trait to support position tracking while keeping domain model clean

**Rationale**: The `DocCodec` trait (defined in `src/codec/mod.rs`) is the proper extension point for adding position tracking. All parsers implement this trait, so extending it ensures uniform position tracking. **Critically, position data stays in the codec/builder layers and does NOT pollute BeliefNode** (which is a domain model).

**Architecture**: Position tracking happens in three places:
1. **DocCodec implementations** - Track positions during parsing, store internally
2. **GraphBuilder** - Builds a `PositionIndex` during `parse_content()` by querying codec
3. **LSP Server** - Queries GraphBuilder's position index for LSP operations

**Changes to `src/codec/mod.rs` (DocCodec trait)**:
- [ ] Add trait method: `fn get_node_range(&self, bid: &Bid) -> Option<Range>`
- [ ] Add trait method: `fn get_link_ranges(&self) -> Vec<LinkPosition>`
- [ ] Add trait method: `fn supports_positions(&self) -> bool { false }` (default: opt-in)
- [ ] Document position tracking contract in trait documentation
- [ ] Position data is codec-internal - NOT stored in ProtoBeliefNode or BeliefNode

**Changes to `src/codec/diagnostic.rs`**:
- [ ] Add `range: Option<Range>` field to `ParseDiagnostic`
- [ ] Update diagnostic generation to include ranges when available
- [ ] Add conversion utility: `lsp_types::Range` ↔ internal range type (if needed)

**Changes to codec implementations**:
- [ ] Update `src/codec/md.rs` (MdCodec):
  - Add internal `positions: HashMap<Bid, Range>` field
  - Track heading positions during parsing (line/column of `#` markers)
  - Track link positions (source range of `[text](ref)`)
  - Track BID annotation positions
  - Implement `get_node_range()` and `get_link_ranges()`
  - Return `true` for `supports_positions()`
- [ ] Update `src/codec/lattice_toml.rs` (TomlCodec):
  - Add internal position tracking for frontmatter blocks
  - Track individual TOML field positions if possible
  - Implement position query methods
- [ ] Position data lives only in codec instances, never in ProtoBeliefNode or BeliefNode

**Changes to `src/codec/builder.rs` (GraphBuilder)**:
- [ ] Add `position_index: Option<PositionIndex>` field to GraphBuilder
- [ ] During `parse_content()`, after codec.parse():
  - Query `codec.get_node_range()` for each parsed BID
  - Build PositionIndex mapping BID ↔ Range
  - Store in `self.position_index`
- [ ] Add method: `pub fn position_index(&self) -> Option<&PositionIndex>`
- [ ] Add helper: `pub fn get_node_at_position(&self, line: u32, col: u32) -> Option<Bid>`

**Changes to `src/codec/position.rs` (NEW FILE)**:
- [ ] Create `PositionIndex` struct with BID ↔ Range mappings
- [ ] Implement efficient position queries (interval tree or simple lookup)
- [ ] Provide conversion utilities for lsp_types::Range

**Testing**:
- [ ] Test: MdCodec tracks positions internally, query methods return correct ranges
- [ ] Test: TomlCodec tracks frontmatter positions
- [ ] Test: GraphBuilder builds PositionIndex during parse_content
- [ ] Test: `get_node_at_position()` returns correct BID for various positions
- [ ] Test: Diagnostic ranges point to correct source locations
- [ ] Test: Custom codec can opt-out (supports_positions returns false, no crash)
- [ ] Test: BeliefNode remains unchanged (no Range field)

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

**Coordination with daemon**:
- [ ] Share `DaemonService` instance between LSP server and file watcher
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

### 7. Documentation and Examples (0.5 days)

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

1. **Incremental document sync in Issue 11 or defer to Issue 12?**
   - **Decision**: Defer to Issue 12. Use full-document sync (TextDocumentSyncKind::FULL) for simplicity. Most editors handle this fine for markdown documents.

2. **Should LSP server share DaemonService instance or create its own?**
   - **Decision**: Share instance. Allows coordination between filesystem changes and editor changes. Document synchronization semantics carefully.

3. **How to handle documents with no BIDs yet?**
   - **Decision**: LSP works fine without BIDs. Hover shows "No BID yet" and suggests running `noet parse` to inject BIDs.

4. **Support for both .md and .toml files in LSP?**
   - **Decision**: Yes. Register LSP for both file types. Document how to configure editor to use noet for both.

5. **How to distribute VSCode extension?**
   - **Decision**: For v0.2.0, document manual installation. For v0.3.0+, consider publishing to VSCode marketplace.

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

**Decision 3: Position tracking in BeliefNode**
- Date: [To be filled during implementation]
- Rationale: Positions are integral to node identity in LSP context. Store with node for easy access.
- Alternative: Separate index mapping BID → Range (more complex, less ergonomic)

**Decision 4: LSP as separate binary (noet-lsp)**
- Date: [To be filled during implementation]
- Rationale: IDE spawns LSP server, easier to debug as separate process. Could be `noet lsp` subcommand instead.
- Decision: Start as subcommand (`noet lsp`), easy to split later if needed

## References

- **Depends On**: [`ISSUE_10_DAEMON_TESTING.md`](./ISSUE_10_DAEMON_TESTING.md) - daemon must be working
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
  - `src/codec/builder.rs` - add position tracking in order to construct diagnostics with this information
  - `src/codec/diagnostic.rs` - add ranges to diagnostics
  - `src/properties.rs` - add Range to BeliefNode
  - `src/bin/noet-lsp.rs` - new LSP server binary
  - `Cargo.toml` - add tower-lsp, lsp-types dependencies
- **New Files**:
  - `docs/lsp.md` - LSP documentation
  - `.vscode/extensions/noet/` - VSCode extension
  - Examples of editor configurations (Zed, Neovim)
