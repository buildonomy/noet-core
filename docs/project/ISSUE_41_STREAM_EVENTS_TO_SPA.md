# Issue 41: Stream BeliefEvents to Browser for Incremental SPA Updates

**Priority**: MEDIUM  
**Estimated Effort**: 3-4 days  
**Dependencies**: None (Issue 40 complete)  
**Blocks**: Future collaborative editing features

## Summary

Enable real-time incremental updates in watch mode by streaming `BeliefEvent`s directly to the browser via WebSocket/SSE, eliminating the need to reload `beliefbase.json` on every document change.

## Problem Statement

### Current Architecture Limitations

The watch mode currently regenerates HTML and exports the entire `beliefbase.json` on every file change:

```
File change detected
    ↓
Parse document → Generate BeliefEvents → Update session_bb
    ↓
Regenerate HTML
    ↓
Export entire beliefbase.json (57 states, 66 relations)
    ↓
Browser detects file change (via LiveReload)
    ↓
Browser reloads page
    ↓
SPA loads entire beliefbase.json from scratch
    ↓
Rebuild navigation tree, metadata, etc.
```

**Problems**:
1. **Inefficient**: Exports/loads entire belief base even for single document changes
2. **Slow**: Full page reload + full JSON parse on every change
3. **Disruptive**: Loses scroll position, open panels, selected document
4. **Bandwidth waste**: Transfers entire belief base repeatedly
5. **Doesn't scale**: Will be prohibitive for large belief bases (1000+ documents)

### Desired Architecture

Stream `BeliefEvent`s directly to browser for incremental updates:

```
File change detected
    ↓
Parse document → Generate BeliefEvents → Update session_bb
    ↓                                           ↓
    ↓                                      Pipe to WebSocket
Regenerate affected HTML only                  ↓
    ↓                                      Browser receives events
Browser detects HTML change                    ↓
    ↓                                      Apply events to local BeliefBase
Update only affected view                      ↓
                                           Update only affected UI components
```

**Benefits**:
1. **Fast**: Sub-second updates, no page reload
2. **Efficient**: Only transfer changed data (delta updates)
3. **Smooth UX**: Preserves scroll position, selection, panel state
4. **Scalable**: O(changes) instead of O(total_documents)
5. **Foundation for collaboration**: Multiple users can subscribe to same event stream

## Technical Context

### Existing Infrastructure (Already Available)

1. **BeliefEvent Stream**: `DocumentCompiler` has `tx` channel that broadcasts all events
   - `BeliefEvent::NodeUpdate`
   - `BeliefEvent::RelationChange`
   - `BeliefEvent::PathsAdded`
   - etc.

2. **Watch Server**: `noet watch --serve` already runs HTTP server
   - Serves static HTML/CSS/JS
   - Could easily add WebSocket endpoint

3. **SPA BeliefBase**: `viewer.js` already maintains client-side belief base
   - Loads from `beliefbase.json` on startup
   - Has navigation tree, metadata panel logic
   - Just needs event application logic

### What's Missing

1. **WebSocket Endpoint**: Need to add WebSocket handler to watch server
2. **Event Serialization**: BeliefEvents need to serialize to JSON for wire transfer
3. **Client Event Handler**: SPA needs to apply incoming events to local state
4. **Selective HTML Regen**: Only regenerate HTML for documents that changed
5. **Graceful Reconnection**: Handle WebSocket disconnects, full sync on reconnect

## Goals

1. Add WebSocket endpoint to `noet watch --serve` that streams BeliefEvents
2. Serialize BeliefEvents to JSON format suitable for browser consumption
3. Update SPA (`viewer.js`) to apply incremental BeliefEvent updates
4. Preserve UI state (scroll position, open panels) across updates
5. Optimize HTML regeneration to only process changed documents
6. Handle reconnection gracefully (full sync if connection lost)

## Non-Goals

- **Collaborative editing**: Multiple users editing same document (future)
- **Conflict resolution**: CRDT/OT for concurrent edits (future)
- **Authentication**: Secure multi-user access (future)
- **Persistence**: Store events for replay/undo (future, maybe ISSUE_16 integration)

## Open Questions

1. **WebSocket vs. SSE**: WebSocket allows bidirectional, SSE is simpler. Which to use?
   - Recommendation: Start with SSE (simpler), upgrade to WebSocket if needed for bidirectional
   
2. **Event Batching**: Should we batch multiple events from single parse?
   - Recommendation: Yes, batch events within 100ms window to reduce churn

3. **Full Sync Trigger**: When should browser request full beliefbase.json reload?
   - On initial connection
   - On reconnection after disconnect
   - If event application fails (schema mismatch)

4. **Backward Compatibility**: Should we keep full reload as fallback?
   - Recommendation: Yes, detect WebSocket availability and gracefully degrade

## Success Criteria

- [ ] Watch server exposes WebSocket/SSE endpoint at `/events`
- [ ] BeliefEvents serialize to JSON and stream to connected browsers
- [ ] SPA applies events incrementally without page reload
- [ ] Document edits reflect in browser within 1 second
- [ ] Scroll position and UI state preserved across updates
- [ ] Connection loss triggers full sync on reconnect
- [ ] No regression in existing watch mode functionality
- [ ] Memory usage remains stable over long watch sessions

## Risks

### Risk 1: Event Ordering
**Risk**: Out-of-order event delivery could corrupt client state  
**Mitigation**: Add sequence numbers to events, detect gaps, trigger full sync if needed

### Risk 2: Client State Drift
**Risk**: Bug in client event application → client state diverges from server  
**Mitigation**: Periodic checksums, full sync on mismatch detection

### Risk 3: Memory Leaks
**Risk**: Long-running watch session accumulates events/state in browser  
**Mitigation**: Periodic garbage collection, evict old events, test multi-hour sessions

## Related Issues

- **ISSUE_40**: Network index generation (provides foundation - events now flow correctly)
- **ISSUE_16**: Automerge integration (future: event persistence/replay)
- **ISSUE_34**: BeliefBase balancing (ensures consistent state for event stream)

## Architecture Notes

### Event Flow

```
GraphBuilder.parse_content()
    ↓
Generates BeliefEvents
    ↓
builder.tx().send(event) ← Multiple subscribers possible!
    ↓         ↓
    ↓         WebSocket handler → Browser (NEW)
    ↓
DocumentCompiler.session_bb.process_event()
```

### Key Insight

The `tx` channel is **already multi-subscriber** via `broadcast` channel. We just need to add a WebSocket task that subscribes to the same stream. No changes to parsing logic needed!

### Relevant Code

- `src/codec/compiler.rs`: `DocumentCompiler` with `tx` channel
- `src/codec/builder.rs`: `GraphBuilder` that sends events
- `src/watch.rs`: Watch mode server (where WebSocket endpoint goes)
- `pkg/viewer.js`: SPA that needs event handler

## Implementation Sketch

### 1. Add WebSocket Endpoint (Rust)

```rust
// In watch.rs
async fn handle_websocket(
    ws: WebSocketUpgrade,
    State(event_rx): State<broadcast::Receiver<BeliefEvent>>,
) -> impl IntoResponse {
    ws.on_upgrade(move |socket| stream_events(socket, event_rx))
}

async fn stream_events(socket: WebSocket, mut rx: broadcast::Receiver<BeliefEvent>) {
    while let Ok(event) = rx.recv().await {
        let json = serde_json::to_string(&event)?;
        socket.send(Message::Text(json)).await?;
    }
}
```

### 2. Apply Events in SPA (JavaScript)

```javascript
// In viewer.js
const eventSource = new EventSource('/events');
eventSource.onmessage = (msg) => {
    const event = JSON.parse(msg.data);
    applyEventToBeliefBase(event);
    updateAffectedViews(event);
};

function applyEventToBeliefBase(event) {
    switch (event.type) {
        case 'NodeUpdate':
            beliefBase.states[event.bid] = event.node;
            break;
        case 'RelationChange':
            // Update relation graph
            break;
        // ... other event types
    }
}
```

## Future Enhancements

- **Collaborative editing**: Multiple browsers editing same document
- **Event persistence**: Store events for replay/undo (Automerge integration)
- **Optimistic updates**: Apply local changes immediately, reconcile with server
- **Conflict resolution**: CRDT-based merge for concurrent edits

---

**Next Steps**: 
1. Confirm WebSocket vs. SSE decision
2. Define JSON schema for serialized BeliefEvents
3. Implement WebSocket endpoint in watch server
4. Update SPA to handle event stream
5. Test with multi-file edits, long sessions