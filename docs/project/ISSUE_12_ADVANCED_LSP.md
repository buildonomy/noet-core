# Issue 12: Advanced Language Server Protocol (LSP) Features

**Priority**: MEDIUM - Enhances IDE experience for v0.3.0+
**Estimated Effort**: 5-7 days
**Dependencies**: Issue 11 (Basic LSP must be working)
**Target Version**: v0.3.0 (post-announcement, enhancement release)
**Context**: Advanced LSP features make noet a truly first-class editing experience

## Summary

Implement advanced Language Server Protocol features for noet, including navigation (go to definition, find references, document outline), editing assistance (autocompletion, formatting, code actions), and performance optimizations (incremental sync). This transforms the basic LSP from Issue 11 into a comprehensive language tooling experience comparable to modern IDEs for programming languages.

**User Experience**: Users can click on `[[links]]` to jump to definitions, autocomplete references as they type, see document outlines in the sidebar, format documents with one keystroke (injecting BIDs, fixing links), apply quick fixes for unresolved references, and rename nodes with automatic reference updates across all documents.

**Post-Implementation**: noet has IDE tooling comparable to programming languages like Rust, TypeScript, or Python.

## Goals

1. **Navigation Features**:
   - Go to definition on `[[links]]` and BIDs
   - Find all references to a node
   - Document outline/symbol hierarchy in sidebar
   - Clickable document links
   - Workspace-wide symbol search

2. **Editing Features**:
   - Autocomplete for `[[references]]` and node titles
   - Format document (inject BIDs, normalize links)
   - Code actions for quick fixes (resolve references, create missing nodes)
   - Rename symbol with automatic reference updates

3. **Performance Optimizations**:
   - Incremental document synchronization
   - Lazy parsing for large documents
   - Debounced diagnostics

4. **Quality of Life**:
   - Semantic tokens (syntax highlighting for BIDs, links)
   - Inlay hints (show resolved targets inline)
   - Document links (clickable URLs and file paths)

## Architecture

### Extended LSP Capabilities

Building on Issue 11, add:

```rust
impl LanguageServer for NoetLanguageServer {
    // === Issue 11 (already implemented) ===
    // initialize, didOpen, didChange, hover, publishDiagnostics
    
    // === Issue 12 (new) ===
    
    // Navigation
    async fn goto_definition(&self, params: GotoDefinitionParams) 
        -> Result<Option<GotoDefinitionResponse>>;
    
    async fn references(&self, params: ReferenceParams) 
        -> Result<Option<Vec<Location>>>;
    
    async fn document_symbol(&self, params: DocumentSymbolParams) 
        -> Result<Option<DocumentSymbolResponse>>;
    
    async fn document_link(&self, params: DocumentLinkParams) 
        -> Result<Option<Vec<DocumentLink>>>;
    
    async fn workspace_symbol(&self, params: WorkspaceSymbolParams) 
        -> Result<Option<Vec<SymbolInformation>>>;
    
    // Editing
    async fn completion(&self, params: CompletionParams) 
        -> Result<Option<CompletionResponse>>;
    
    async fn formatting(&self, params: DocumentFormattingParams) 
        -> Result<Option<Vec<TextEdit>>>;
    
    async fn code_action(&self, params: CodeActionParams) 
        -> Result<Option<CodeActionResponse>>;
    
    async fn rename(&self, params: RenameParams) 
        -> Result<Option<WorkspaceEdit>>;
    
    async fn prepare_rename(&self, params: TextDocumentPositionParams) 
        -> Result<Option<PrepareRenameResponse>>;
    
    // Performance
    async fn semantic_tokens_full(&self, params: SemanticTokensParams) 
        -> Result<Option<SemanticTokensResult>>;
}
```

### Data Structures

**Link Index** (for navigation):
```rust
// Track all links in document for fast lookup
pub struct LinkIndex {
    // Map position → link info
    links: BTreeMap<Position, LinkInfo>,
}

pub struct LinkInfo {
    pub range: Range,
    pub target: NodeKey,
    pub resolved: bool,
    pub target_location: Option<Location>,  // cached for performance
}
```

**Symbol Index** (for outline and search):
```rust
pub struct SymbolIndex {
    // Hierarchical document structure
    symbols: Vec<DocumentSymbol>,
    // Flat index for fast lookup
    symbol_map: HashMap<Bid, SymbolInformation>,
}
```

**Completion Cache**:
```rust
pub struct CompletionCache {
    // All available nodes for autocomplete
    nodes: Vec<CompletionItem>,
    // Indexed by prefix for fast lookup
    prefix_index: HashMap<String, Vec<usize>>,
    // Last update time
    updated_at: SystemTime,
}
```

## Implementation Steps

### Phase 1: Navigation Features (2-3 days)

#### 1.1. Go to Definition (0.5 days)

**Objective**: Jump to target when clicking on `[[links]]` or BIDs

**Implementation**:
- [ ] Parse link syntax at cursor position
- [ ] Extract target (NodeKey or BID)
- [ ] Resolve to target location (file, line, column)
- [ ] Return `Location` or `LocationLink` with target range
- [ ] Handle multiple definitions (e.g., ambiguous references)
- [ ] Handle unresolved references gracefully (show error)

**Edge cases**:
- [ ] Link to heading within same document
- [ ] Link to heading in different document
- [ ] Link with anchor: `[[doc#heading]]`
- [ ] Link with BID: `[[doc|bid:12345678]]`
- [ ] Unresolved link (show diagnostic)

**Testing**:
- [ ] Click on link jumps to correct location
- [ ] Click on BID jumps to node
- [ ] Click on unresolved link shows error message
- [ ] Cross-document links work correctly

#### 1.2. Find All References (0.5 days)

**Objective**: Find all places that reference a node

**Implementation**:
- [ ] Get node at cursor position (heading or link)
- [ ] Extract node's BID
- [ ] Query all documents for links to this BID
- [ ] Return list of `Location`s
- [ ] Include references in comments/text (optional)
- [ ] Show count in UI ("12 references found")

**Testing**:
- [ ] Find references shows all links to node
- [ ] Find references works across documents
- [ ] Find references includes current location
- [ ] Empty result when no references exist

#### 1.3. Document Symbol / Outline (0.5 days)

**Objective**: Show document structure in sidebar

**Implementation**:
- [ ] Extract heading hierarchy from document
- [ ] Convert to `DocumentSymbol` tree structure
- [ ] Include heading level, title, range
- [ ] Include links as children (optional)
- [ ] Support folding regions
- [ ] Update when document changes

**Symbol kinds**:
- [ ] Headings → `SymbolKind::Heading` or `SymbolKind::Namespace`
- [ ] Links → `SymbolKind::Field`
- [ ] BIDs → `SymbolKind::Key`

**Testing**:
- [ ] Outline shows all headings
- [ ] Outline hierarchy matches document structure
- [ ] Clicking outline jumps to heading
- [ ] Outline updates when document changes

#### 1.4. Document Links (0.5 days)

**Objective**: Make `[[links]]` clickable in editor

**Implementation**:
- [ ] Parse all links in document
- [ ] Convert to `DocumentLink` with range and target
- [ ] Resolve target to file:// URL
- [ ] Handle external URLs (http://, https://)
- [ ] Handle file paths (relative, absolute)

**Testing**:
- [ ] Ctrl+click on link opens target document
- [ ] External URLs open in browser
- [ ] File paths open in editor
- [ ] Unresolved links show as plain text

#### 1.5. Workspace Symbol Search (0.5 days)

**Objective**: Search for nodes across entire workspace

**Implementation**:
- [ ] Index all nodes in workspace
- [ ] Support fuzzy search by title
- [ ] Return list of `SymbolInformation`
- [ ] Include file location and symbol kind
- [ ] Update index when files change
- [ ] Limit results to top 100 for performance

**Testing**:
- [ ] Search finds nodes by full title
- [ ] Search finds nodes by partial match
- [ ] Search works across all documents
- [ ] Search results are ranked by relevance

---

### Phase 2: Editing Features (2-3 days)

#### 2.1. Completion Provider (1 day)

**Objective**: Autocomplete `[[references]]` and node titles

**Implementation**:
- [ ] Detect completion trigger: `[[` typed
- [ ] Build list of available nodes (from cache)
- [ ] Filter by prefix match
- [ ] Return `CompletionItem` list with:
  - Label: node title
  - Detail: BID, file path
  - Kind: reference, heading, document
  - Insert text: `[[target]]` or `[[target|bid:xxx]]`
- [ ] Support fuzzy matching
- [ ] Rank by relevance (recent, proximity, popularity)

**Completion triggers**:
- [ ] `[[` → show all nodes
- [ ] `[[tex` → show nodes matching "tex"
- [ ] `[[doc#` → show headings in "doc"

**Testing**:
- [ ] Typing `[[` shows completion list
- [ ] Completion filters as you type
- [ ] Selecting completion inserts correct link
- [ ] Completion works across documents

#### 2.2. Document Formatting (0.5 days)

**Objective**: Format document (inject BIDs, normalize links)

**Implementation**:
- [ ] Implement `textDocument/formatting`
- [ ] Use existing BID injection logic from parser
- [ ] Normalize link syntax: `[[title]]` → `[[title|bid:xxx]]`
- [ ] Fix broken links (if target found)
- [ ] Sort frontmatter keys (optional)
- [ ] Ensure idempotent (format twice = same result)

**Format operations**:
- [ ] Inject missing BIDs
- [ ] Add BIDs to links for stability
- [ ] Update link titles if `auto_title: true`
- [ ] Normalize whitespace (optional)

**Testing**:
- [ ] Format injects BIDs
- [ ] Format normalizes links
- [ ] Format is idempotent
- [ ] Format preserves content

#### 2.3. Code Actions / Quick Fixes (0.5 days)

**Objective**: Provide quick fixes for diagnostics

**Implementation**:
- [ ] Implement `textDocument/codeAction`
- [ ] For `UnresolvedReference` diagnostic:
  - Action: "Create missing document"
  - Action: "Remove broken link"
  - Action: "Search for similar nodes"
- [ ] For missing BID:
  - Action: "Inject BID"
- [ ] For outdated link title:
  - Action: "Update link title"

**Code action kinds**:
- [ ] `quickfix` - fix problems
- [ ] `refactor` - restructure code
- [ ] `source.organizeImports` - organize links (future)

**Testing**:
- [ ] Quick fix creates missing document
- [ ] Quick fix removes broken link
- [ ] Quick fix injects BID
- [ ] Quick fix updates link title

#### 2.4. Rename Symbol (1 day)

**Objective**: Rename node and update all references

**Implementation**:
- [ ] Implement `textDocument/prepareRename` - validate rename
- [ ] Implement `textDocument/rename` - perform rename
- [ ] Find all references to node (reuse from 1.2)
- [ ] Create `WorkspaceEdit` with:
  - Update node title in source document
  - Update all link texts referencing this node
  - Update file name if node is document (optional)
- [ ] Handle conflicts (duplicate names)
- [ ] Preview changes before applying

**Rename scenarios**:
- [ ] Rename heading → update link titles
- [ ] Rename document → update file path references
- [ ] Rename with conflicts → show warning

**Testing**:
- [ ] Rename updates all references
- [ ] Rename works across documents
- [ ] Rename handles conflicts gracefully
- [ ] Rename preview shows all changes

---

### Phase 3: Performance Optimizations (1 day)

#### 3.1. Incremental Document Sync (0.5 days)

**Objective**: Only re-parse changed portions of document

**Implementation**:
- [ ] Switch from `TextDocumentSyncKind::FULL` to `INCREMENTAL`
- [ ] Receive `TextDocumentContentChangeEvent` with range
- [ ] Apply delta to in-memory document
- [ ] Re-parse only affected sections (heading and below)
- [ ] Update indices incrementally
- [ ] Benchmark performance improvement

**Testing**:
- [ ] Incremental sync produces same result as full sync
- [ ] Performance improvement for large documents
- [ ] Edge cases: delete heading, insert heading, change link

#### 3.2. Debounced Diagnostics (0.25 days)

**Objective**: Don't re-parse on every keystroke

**Implementation**:
- [ ] Add debounce delay (200-500ms) before re-parsing
- [ ] Cancel pending parse if new change arrives
- [ ] Show "parsing..." indicator in status bar (optional)
- [ ] Ensure diagnostics clear immediately on fix

**Testing**:
- [ ] Diagnostics don't appear on every keystroke
- [ ] Diagnostics appear after pause
- [ ] Fixed diagnostics clear immediately

#### 3.3. Lazy Parsing (0.25 days)

**Objective**: Don't parse documents until needed

**Implementation**:
- [ ] Parse on `didOpen` (required)
- [ ] Don't parse on file watcher events if document not open
- [ ] Parse on demand for navigation (go to definition)
- [ ] Cache parse results for frequently accessed documents

**Testing**:
- [ ] Closed documents aren't parsed automatically
- [ ] Go to definition triggers parse if needed
- [ ] Open documents are always up-to-date

---

### Phase 4: Quality of Life Features (1 day)

#### 4.1. Semantic Tokens (0.5 days)

**Objective**: Syntax highlighting for BIDs, links, special syntax

**Implementation**:
- [ ] Implement `textDocument/semanticTokens/full`
- [ ] Define token types: `bid`, `link`, `heading`, `keyword`
- [ ] Define token modifiers: `resolved`, `unresolved`
- [ ] Colorize based on token type
- [ ] Update on document change

**Token legend**:
- [ ] BIDs: `bid:12345678` → special color
- [ ] Resolved links: `[[target]]` → link color
- [ ] Unresolved links: `[[broken]]` → error color
- [ ] Headings: `# Title` → heading color

**Testing**:
- [ ] BIDs highlighted correctly
- [ ] Links highlighted based on resolution status
- [ ] Syntax highlighting updates on change

#### 4.2. Inlay Hints (0.5 days)

**Objective**: Show resolved target inline

**Implementation**:
- [ ] Implement `textDocument/inlayHint`
- [ ] Show resolved target after link: `[[link]] → path/to/doc.md#heading`
- [ ] Show BID on hover (if not visible)
- [ ] Make hints toggleable in editor settings

**Testing**:
- [ ] Inlay hints show resolved paths
- [ ] Inlay hints don't clutter UI
- [ ] Inlay hints toggle on/off

---

## Testing Requirements

### Unit Tests
- Link parsing and resolution
- Symbol extraction from document
- Completion filtering and ranking
- Rename reference finding
- Incremental sync delta application

### Integration Tests
- End-to-end navigation flow
- End-to-end editing flow (autocomplete → format → save)
- Rename across multiple documents
- Performance benchmarks (incremental vs full sync)

### Manual Testing in IDEs
- VSCode: Test all features work correctly
- Zed: Test all features work correctly
- Neovim: Test all features work correctly
- Test with real-world document sets (100+ documents)
- Test with large documents (>10,000 lines)

### Performance Benchmarks
- Incremental sync faster than full sync for large docs
- Completion response time < 50ms
- Go to definition response time < 100ms
- Rename across 100 documents < 2 seconds

## Success Criteria

- [ ] Go to definition works on links and BIDs
- [ ] Find all references finds all usages
- [ ] Document outline shows heading hierarchy
- [ ] Workspace symbol search finds nodes across workspace
- [ ] Autocomplete suggests available references
- [ ] Format injects BIDs and normalizes links
- [ ] Code actions provide quick fixes for diagnostics
- [ ] Rename updates all references across documents
- [ ] Incremental sync improves performance
- [ ] All features tested in at least 2 IDEs
- [ ] Performance benchmarks meet targets
- [ ] Documentation updated with new features

## Risks

**Risk**: Navigation performance poor on large workspaces
**Mitigation**: Implement caching, lazy parsing, incremental indexing; benchmark early with large test corpus

**Risk**: Rename breaks document structure
**Mitigation**: Extensive testing; validation before applying; support undo; preview changes

**Risk**: Incremental sync bugs cause corruption
**Mitigation**: Validate incremental result matches full parse; fallback to full parse on error; extensive testing

**Risk**: Completion results too noisy
**Mitigation**: Implement ranking/relevance; limit to top N results; allow filtering by document/type

**Risk**: Code actions too complex for users
**Mitigation**: Clear action descriptions; preview changes; make actions reversible

**Risk**: Feature scope creep delays release
**Mitigation**: Prioritize navigation over editing features; ship incrementally if needed; defer semantic tokens to v0.4.0

## Open Questions

1. **Should autocomplete include node content preview?**
   - Option A: Just title and BID (simple, fast)
   - Option B: Include first paragraph of content (helpful, slower)
   - **Decision**: TBD during implementation based on performance

2. **Rename: Update file names or just content?**
   - Option A: Just content (safer, simpler)
   - Option B: Update file names too (powerful, complex)
   - **Decision**: TBD - start with content only, add file rename in v0.4.0

3. **Should format be automatic on save or manual?**
   - Option A: Manual only (user control)
   - Option B: Automatic with opt-out (convenience)
   - **Decision**: TBD - provide both options, default to manual

4. **How aggressive should fuzzy matching be?**
   - Option A: Strict prefix matching (predictable)
   - Option B: Aggressive fuzzy (more results, noisier)
   - **Decision**: TBD - test both with users

5. **Should we support language server workspace configuration?**
   - Option A: Configuration via workspace/configuration requests
   - Option B: Configuration via .noet/config.toml
   - **Decision**: TBD - both if possible, prioritize .noet/config.toml

## Future Work (Post v0.3.0)

**Refactoring support**:
- Extract heading to new document
- Merge documents
- Split document at cursor
- Convert inline link to reference

**Advanced navigation**:
- Call hierarchy (document dependency graph)
- Type hierarchy (schema relationships)
- Breadcrumbs (current location path)

**Collaborative editing**:
- Multiple clients on same document
- Conflict resolution
- Live cursors

**AI integration**:
- Suggest related documents
- Generate document summaries
- Auto-tag with metadata

## Decision Log

**Decision 1: Incremental sync in Issue 12**
- Date: [To be filled during implementation]
- Rationale: Performance optimization important for large documents, but not critical for initial release
- Impact: Better performance for large documents

**Decision 2: Navigation features prioritized over editing**
- Date: [To be filled during implementation]
- Rationale: Navigation has higher ROI - most frequently used features
- Impact: Ship navigation first if time constrained

**Decision 3: Use completion cache with TTL**
- Date: [To be filled during implementation]
- Rationale: Autocomplete needs to be fast (<50ms), caching essential
- Alternative: Re-query on every request (too slow)

**Decision 4: Rename as workspace edit, not file operations**
- Date: [To be filled during implementation]
- Rationale: Safer to update content only, file renames can be manual
- Future: Add file rename in v0.4.0 with more testing

## References

- **Depends On**: [`ISSUE_11_BASIC_LSP.md`](./ISSUE_11_BASIC_LSP.md) - basic LSP must be working
- **Roadmap**: To be added to v0.3.0 section of roadmap
- **LSP Specification**: 
  - Navigation: https://microsoft.github.io/language-server-protocol/specifications/lsp/3.17/specification/#textDocument_definition
  - Completion: https://microsoft.github.io/language-server-protocol/specifications/lsp/3.17/specification/#textDocument_completion
  - Formatting: https://microsoft.github.io/language-server-protocol/specifications/lsp/3.17/specification/#textDocument_formatting
- **tower-lsp Examples**: https://github.com/ebkalderon/tower-lsp/tree/master/examples
- **Reference Implementations**:
  - rust-analyzer completion: https://github.com/rust-lang/rust-analyzer/tree/master/crates/ide-completion
  - marksman navigation: https://github.com/artempyanykh/marksman
- **Code Changes**:
  - `src/bin/noet-lsp.rs` - implement all new LSP methods
  - `src/daemon.rs` - add indexing for navigation/completion
  - `src/codec/parser.rs` - incremental parsing support
  - `src/query/mod.rs` - optimize for LSP queries
- **Performance Tools**:
  - `cargo flamegraph` for profiling
  - `criterion` for benchmarking
  - LSP inspector tools for debugging