# Issue 41: Query Builder UI for Interactive Viewer

**Priority**: MEDIUM
**Estimated Effort**: 4-5 days
**Dependencies**: ISSUE_39 (Phase 1 complete), ISSUE_40 (network indices working)
**Version**: 0.1

## Summary

Add a visual query builder UI to the interactive HTML viewer that allows users to construct and execute queries against the belief base without writing code. The query builder translates user selections into WASM API calls and displays results in the metadata panel.

## Goals

- Visual query construction (dropdowns, checkboxes, text inputs)
- Support common query patterns (by kind, by schema, by relationship)
- Real-time result preview
- Export query results (JSON, CSV)
- Integration with metadata panel for result exploration
- No code required (accessible to non-technical users)

## Architecture

### UI Components

**Query Builder Panel**:
- Toggle button in header: "Query" (opens builder panel)
- Positioned above/beside metadata panel (desktop) or drawer (mobile)
- Collapsible when not in use
- Persistent state (remember last query)

**Query Modes**:
1. **Simple Mode**: Pre-defined query templates
   - "Find all documents"
   - "Find all sections in current network"
   - "Find nodes linking to current document"
   - "Find broken links"

2. **Advanced Mode**: Custom query construction
   - Filter by Kind (Belief, Network, Section, etc.)
   - Filter by Schema (if present)
   - Filter by Network
   - Filter by Relationship (backlinks, forward links, related nodes)
   - Combine filters with AND/OR logic

3. **Raw Mode**: Direct WASM query API access (for power users)
   - Text input for query DSL
   - Syntax highlighting
   - Query validation

### WASM Integration

**Required API Methods** (may already exist):
```rust
impl BeliefBaseWasm {
    // Query by kind
    pub fn query_by_kind(&self, kind: String) -> JsValue;
    
    // Query by schema
    pub fn query_by_schema(&self, schema: String) -> JsValue;
    
    // Query by network
    pub fn query_by_network(&self, network_bid: String) -> JsValue;
    
    // Query by relationship
    pub fn query_related(&self, bid: String, relation_type: String) -> JsValue;
    
    // Combined query
    pub fn query(&self, query_json: String) -> JsValue;
}
```

**Query DSL** (JSON format):
```json
{
  "filters": [
    { "type": "kind", "value": "Belief" },
    { "type": "network", "value": "1f100f54-..." },
    { "type": "schema", "value": "Task" }
  ],
  "logic": "AND",
  "limit": 100,
  "offset": 0
}
```

## Implementation Steps

### Phase 1: Simple Mode (2 days)

#### Step 1.1: Query Builder UI Shell (4 hours)
- [ ] Add query builder toggle button to header
- [ ] Create collapsible query panel (CSS + JS)
- [ ] Panel positioning (desktop: left of metadata, mobile: drawer)
- [ ] Panel state management (localStorage persistence)

#### Step 1.2: Template-Based Queries (6 hours)
- [ ] Define query templates (JSON config)
- [ ] Dropdown selector for templates
- [ ] Execute template via WASM API
- [ ] Display results in results panel (below query builder)

#### Step 1.3: Result Display (4 hours)
- [ ] Results table: Title, Kind, Schema, Path
- [ ] Click result → open in metadata panel
- [ ] Pagination (20 results per page)
- [ ] Result count display

### Phase 2: Advanced Mode (2 days)

#### Step 2.1: Filter UI Components (6 hours)
- [ ] Kind dropdown (populate from WASM API)
- [ ] Schema dropdown (populate from WASM API)
- [ ] Network dropdown (populate from WASM API)
- [ ] Add/remove filter buttons
- [ ] AND/OR logic toggle

#### Step 2.2: Query Execution (4 hours)
- [ ] Build query DSL from filter selections
- [ ] Execute via WASM `query()` method
- [ ] Handle errors (invalid query, WASM failure)
- [ ] Loading indicator

#### Step 2.3: Query Refinement (4 hours)
- [ ] Live result count preview
- [ ] Filter validation (disable invalid combinations)
- [ ] Clear all filters button
- [ ] Save query button (localStorage)

### Phase 3: Export & Polish (1 day)

#### Step 3.1: Export Functionality (3 hours)
- [ ] Export to JSON (download file)
- [ ] Export to CSV (download file)
- [ ] Copy results to clipboard (formatted)

#### Step 3.2: UX Improvements (3 hours)
- [ ] Keyboard shortcuts (`Ctrl+K` to open query builder)
- [ ] Query history (last 5 queries, dropdown)
- [ ] Accessibility (ARIA labels, keyboard navigation)

#### Step 3.3: Documentation (2 hours)
- [ ] In-app help text (tooltips, examples)
- [ ] Update `interactive_viewer.md` design doc
- [ ] User guide section in docs

## Testing Requirements

### Automated Tests

**Browser Tests** (`tests/browser/test_runner.html`):
- [ ] Query builder panel opens/closes
- [ ] Template queries execute successfully
- [ ] Filter UI builds valid query DSL
- [ ] Results display correctly
- [ ] Pagination works
- [ ] Export functions generate valid files

**Integration Tests**:
- [ ] WASM query API returns expected results
- [ ] Query DSL parsing handles edge cases
- [ ] Large result sets render without freezing

### Manual Testing

**Desktop**:
- [ ] Query builder panel positioning
- [ ] Filter dropdowns populate correctly
- [ ] Query execution with various filters
- [ ] Result navigation to metadata panel
- [ ] Export to JSON/CSV
- [ ] Keyboard shortcuts

**Mobile**:
- [ ] Query builder drawer behavior
- [ ] Touch-friendly filter controls
- [ ] Results scrollable
- [ ] Export functionality

## Success Criteria

- [ ] Simple mode templates execute correctly
- [ ] Advanced mode supports all filter types
- [ ] Results display with pagination
- [ ] Export to JSON/CSV works
- [ ] Query state persists across sessions
- [ ] Keyboard shortcuts functional
- [ ] All automated tests pass
- [ ] Manual testing confirms UX quality

## Risks

### Risk 1: WASM Query API Performance
**Problem**: Large queries may freeze browser

**Mitigation**: 
- Web Worker for query execution (non-blocking)
- Streaming results (incremental display)
- Query timeout (abort after 10 seconds)

### Risk 2: Query DSL Complexity
**Problem**: Advanced queries may be too complex for UI representation

**Mitigation**:
- Start with simple filters (80% use case)
- Add Raw Mode for power users
- Provide query examples

### Risk 3: Mobile UX Constraints
**Problem**: Query builder UI may be cramped on small screens

**Mitigation**:
- Simplified mobile UI (fewer options visible)
- Collapsible filter sections
- Full-screen mode option

## Design Decisions

### Query DSL Format
**Decision**: Use JSON format for queries

**Rationale**: 
- Easy to serialize/deserialize
- Human-readable for debugging
- Compatible with WASM API

### Result Display Location
**Decision**: Inline results panel (below query builder)

**Rationale**:
- Keep context visible (don't replace metadata panel)
- Users can compare results while refining query
- Click result to open in metadata panel (exploration pattern)

### Template vs Advanced Mode
**Decision**: Default to Simple Mode (templates)

**Rationale**:
- Most users need common queries
- Advanced mode available for power users
- Progressive disclosure (simple → advanced → raw)

## References

- ISSUE_39: Interactive viewer foundation
- `docs/design/interactive_viewer.md`: § Query Builder UI
- `src/wasm.rs`: WASM API implementation
- `src/query.rs`: Query DSL and execution

## Notes

**Phase Dependencies**:
- Phase 1 (Simple Mode) can be implemented independently
- Phase 2 (Advanced Mode) requires Phase 1 UI foundation
- Phase 3 (Export) depends on Phase 1 + 2 result rendering

**Future Enhancements** (deferred to later issues):
- Saved queries library
- Query sharing via URL parameters
- Query visualization (graph of filters)
- Query performance metrics