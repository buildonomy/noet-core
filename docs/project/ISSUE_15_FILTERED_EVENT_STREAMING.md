# Issue 15: Filtered Event Streaming (Focus Reimagined)

**Priority**: MEDIUM  
**Estimated Effort**: 5-7 days  
**Dependencies**: ISSUE_10 (WatchService), ISSUE_11 (LSP/IPC foundation)  
**Context**: Part of v0.2.0+ roadmap - Advanced real-time collaboration features

## Summary

Implement filtered event streaming to allow consumers to subscribe to a subset of BeliefEvents based on query filters. The old product-specific "focus" concept tracked motivation and UI state. This reimagines "focus" as a **query-filtered event stream**: consumers define their area of interest via a query, and the WatchService transmits only matching events over a bidirectional channel (websocket or similar). This enables efficient real-time updates for clients that only care about specific portions of the graph.

## Goals

1. Enable query-based event filtering at the WatchService level
2. Implement bidirectional event streaming (server → client, client → server)
3. Support multiple concurrent subscriptions with different filters
4. Minimize network overhead by transmitting only relevant events
5. Provide clean API for managing subscriptions
6. Integrate with LSP/IPC protocol (ISSUE_11) or standalone websocket
7. Document patterns for real-time collaboration and incremental sync

## Architecture

### High-Level Flow

```
Client                    WatchService                   Parser/DB
  |                            |                              |
  |-- Subscribe(Query) ------->|                              |
  |                            |-- Register filter            |
  |<-- Ack + Initial State ----|                              |
  |                            |                              |
  |                            |<-- BeliefEvent --------------|
  |                            |-- Filter event               |
  |<-- Filtered BeliefEvent ---|   (matches Query?)           |
  |                            |                              |
  |-- Update BeliefEvent ----->|                              |
  |                            |-- Validate & integrate ----->|
  |<-- Ack/Error --------------|                              |
```

### Query-Based Filtering

Reuse existing `Query`/`PaginatedQuery` infrastructure from `src/query.rs`:

- **Subscription Query**: Defines what events the client cares about
  - By BID (specific nodes)
  - By path pattern (documents in a directory)
  - By relationship (nodes connected to a specific node)
  - By type (only structure updates, only content updates)
  - By network (specific BeliefNetwork roots)

- **Event Matching**: For each incoming `BeliefEvent`, evaluate:
  - Does this event affect any node in the subscription's result set?
  - Does this event create/remove relationships to subscribed nodes?
  - Does this event modify paths matching the subscription?

### Subscription Management

**New Types** (in `src/watch.rs` or new `src/subscription.rs`):

```rust
/// A subscription to filtered events
pub struct EventSubscription {
    id: SubscriptionId,
    query: Query,
    tx: UnboundedSender<FilteredEvent>,
}

/// Unique identifier for a subscription
pub type SubscriptionId = Uuid;

/// An event that passed the subscription filter
pub struct FilteredEvent {
    subscription_id: SubscriptionId,
    event: BeliefEvent,
    matched_nodes: Vec<Bid>, // Which nodes in the query matched
}

/// Manager for active subscriptions
pub struct SubscriptionManager {
    subscriptions: HashMap<SubscriptionId, EventSubscription>,
    // Index for efficient lookup: which subscriptions care about which BIDs
    bid_index: HashMap<Bid, HashSet<SubscriptionId>>,
}
```

**WatchService Integration**:

```rust
impl WatchService {
    /// Subscribe to filtered events matching a query
    pub fn subscribe(
        &mut self,
        query: Query,
    ) -> Result<(SubscriptionId, UnboundedReceiver<FilteredEvent>), BuildonomyError> {
        // Create subscription
        // Evaluate initial query to populate bid_index
        // Return subscription ID and receiver channel
    }

    /// Unsubscribe from filtered events
    pub fn unsubscribe(&mut self, id: SubscriptionId) -> Result<(), BuildonomyError> {
        // Remove subscription
        // Clean up bid_index
    }

    /// Internal: filter and route incoming events to subscriptions
    fn route_event(&self, event: &BeliefEvent) {
        // For each subscription:
        //   - Check if event affects any nodes in subscription query
        //   - Send FilteredEvent to subscriber if match
    }
}
```

### Bidirectional Communication

**Client → Server** (ingest events from clients):

```rust
/// Events that clients can send back to WatchService
pub enum ClientEvent {
    /// Client has modified content
    UpdateContent { path: PathBuf, content: String },
    
    /// Client has created a new node
    CreateNode { proto: ProtoBeliefNode },
    
    /// Client has modified node properties
    UpdateNode { bid: Bid, updates: HashMap<String, Value> },
    
    /// Client has created a relationship
    CreateRelation { from: Bid, to: Bid, rel_type: String },
}

impl WatchService {
    /// Ingest an event from a client and integrate into WatchService
    pub async fn ingest_client_event(
        &self,
        event: ClientEvent,
    ) -> Result<(), BuildonomyError> {
        // Validate event
        // Apply to files (if UpdateContent)
        // Generate BeliefEvent(s)
        // Route through normal FileUpdateSyncer pipeline
        // Broadcast to other subscriptions
    }
}
```

### Transport Layer Options

**Option 1: Websocket** (standalone):
- Use `tokio-tungstenite` or `axum` websocket support
- JSON-serialized events
- Separate from LSP protocol
- Good for web clients, real-time dashboards

**Option 2: LSP Custom Notifications** (integrate with ISSUE_11):
- Use `$/noet/subscribe` and `$/noet/event` notifications
- Integrate with existing LSP server
- Good for editor integrations

**Option 3: IPC/Unix Socket**:
- Local process communication
- Lower latency than websocket
- Good for local tools

**Decision**: Start with **Option 2** (LSP notifications) since ISSUE_11 provides infrastructure. Add websocket support later if needed.

## Implementation Steps

1. **Design Subscription Data Structures** (1 day)
   - [ ] Define `EventSubscription`, `SubscriptionId`, `FilteredEvent` types
   - [ ] Design `SubscriptionManager` with efficient indexing
   - [ ] Define query matching semantics (what events match what queries)
   - [ ] Write tests for subscription creation and removal
   - [ ] Document subscription lifecycle

2. **Implement Event Filtering Logic** (2 days)
   - [ ] Implement `matches_query(event: &BeliefEvent, query: &Query) -> bool`
   - [ ] Handle different query types:
     - BID-based (direct node match)
     - Path-based (file path patterns)
     - Relationship-based (connected nodes)
     - Type-based (structure vs content)
   - [ ] Test filtering with various event types
   - [ ] Optimize for common cases (direct BID lookup)
   - [ ] Document filtering algorithm and edge cases

3. **Integrate with WatchService** (1.5 days)
   - [ ] Add `SubscriptionManager` to `WatchService`
   - [ ] Implement `subscribe()` / `unsubscribe()` methods
   - [ ] Hook into `FileUpdateSyncer` event pipeline
   - [ ] Route events through `SubscriptionManager` before broadcasting
   - [ ] Add subscription metrics (active subs, events routed)
   - [ ] Test with multiple concurrent subscriptions

4. **Implement Bidirectional Communication** (1.5 days)
   - [ ] Define `ClientEvent` enum for inbound events
   - [ ] Implement `ingest_client_event()` validation and routing
   - [ ] Integrate with file writing and parser queue
   - [ ] Prevent event loops (client → server → client)
   - [ ] Add conflict detection for concurrent edits
   - [ ] Test bidirectional flow end-to-end

5. **LSP Protocol Integration** (1 day)
   - [ ] Define LSP custom notifications:
     - `$/noet/subscribe` (client → server)
     - `$/noet/unsubscribe` (client → server)
     - `$/noet/event` (server → client)
     - `$/noet/clientEvent` (client → server)
   - [ ] Implement handlers in LSP server (ISSUE_11)
   - [ ] Add subscription management to LSP session state
   - [ ] Test with LSP client (vscode extension or test harness)

6. **Documentation and Examples** (1 day)
   - [ ] Add module-level docs to `src/subscription.rs`
   - [ ] Document query patterns for common use cases
   - [ ] Create `examples/filtered_streaming.rs` demonstrating:
     - Subscribe to events for specific directory
     - Client sends update, receives confirmation
     - Multiple clients with overlapping subscriptions
   - [ ] Add doctests for subscription API
   - [ ] Update `ISSUE_11` docs to reference filtered streaming

## Testing Requirements

**Unit Tests**:
- `EventSubscription` creation and lifecycle
- Query matching logic for various event types
- BID index maintenance (add/remove subscriptions)
- Event routing to correct subscriptions
- Client event validation

**Integration Tests**:
- End-to-end: subscribe → file change → filtered event received
- Multiple subscriptions with overlapping queries
- Subscription removed → events no longer routed
- Bidirectional: client sends event → integrated → other clients notified
- Concurrent subscriptions with different filters
- Query filters correctly exclude non-matching events

**Performance Tests**:
- Subscription with 1000+ active subscriptions
- Event routing latency (target: <10ms per event)
- Memory usage with many subscriptions
- BID index lookup efficiency

## Success Criteria

- [ ] Consumers can subscribe to filtered event streams via query
- [ ] Only matching events are transmitted to subscribers
- [ ] Multiple concurrent subscriptions work correctly
- [ ] Bidirectional communication: clients can send events back
- [ ] Integrated with LSP protocol (ISSUE_11)
- [ ] Documented API and example code
- [ ] Tests pass for filtering, routing, bidirectional flow
- [ ] Performance acceptable (< 10ms event routing)
- [ ] No memory leaks with long-running subscriptions

## Risks

**Risk**: Query evaluation overhead slows down event processing  
**Mitigation**: Index subscriptions by BID for O(1) lookup; evaluate complex queries lazily; benchmark and optimize hot paths

**Risk**: Event loops (client → server → client infinitely)  
**Mitigation**: Tag events with originating subscription ID; don't echo events back to sender; add loop detection

**Risk**: Subscription memory leaks if clients don't unsubscribe  
**Mitigation**: Implement timeout-based cleanup; add subscription heartbeat; provide admin API to list/force-close subscriptions

**Risk**: Concurrent edit conflicts (two clients modify same node)  
**Mitigation**: Add optimistic locking (version numbers); reject conflicting edits; document conflict resolution strategy

**Risk**: WebSocket vs LSP protocol fragmentation  
**Mitigation**: Start with LSP integration (Option 2); abstract transport layer; add websocket later if needed

## Open Questions

1. **Should subscriptions be persistent across WatchService restarts?**
   - **Leaning No**: Treat as ephemeral session state; clients re-subscribe on reconnect
   - Alternative: Persist to config file for "saved queries"

2. **How to handle subscription queries that match thousands of nodes?**
   - **Approach**: Limit initial result set size; paginate if needed; warn on overly broad queries
   - Alternative: Stream initial state incrementally

3. **Should filtered events include full node state or just deltas?**
   - **Approach**: Send full `BeliefEvent` (includes delta info); client can request full state if needed
   - Alternative: Add `include_full_state: bool` option to subscription

4. **How to handle subscription authorization (who can subscribe to what)?**
   - **Deferred**: No auth in v0.2.0; assume trusted clients
   - Future: Add permission model (ISSUE_16?)

5. **Should subscriptions auto-refresh when query definition changes?**
   - **Leaning No**: Client must explicitly update subscription
   - Alternative: Provide `update_subscription(id, new_query)` method

## Decision Log

**Decision 1: Use LSP Protocol for Initial Implementation**
- Date: [To be filled during implementation]
- Rationale: ISSUE_11 provides LSP infrastructure; good fit for editor integrations
- Impact: Custom notifications `$/noet/subscribe`, `$/noet/event`, etc.
- Deferred: Websocket support for web clients (add in v0.3.0+)

**Decision 2: Subscription Indexing Strategy**
- Date: [To be filled during implementation]
- Approach: Maintain `HashMap<Bid, HashSet<SubscriptionId>>` for fast lookup
- Rebuild index on subscribe/unsubscribe
- Rationale: Most events affect specific BIDs; O(1) lookup critical for performance

**Decision 3: Bidirectional Event Validation**
- Date: [To be filled during implementation]
- Client events must be validated before integration
- Add `originating_subscription_id` to prevent echo loops
- Conflicts detected via optimistic locking (version numbers)

**Decision 4: Query Matching Semantics**
- Date: [To be filled during implementation]
- Event matches if it affects ANY node in the subscription's result set
- Includes: direct updates, relationship changes, path modifications
- Rationale: Consumers need to know about anything affecting their "focus area"

## References

- **Depends On**: 
  - [`ISSUE_10_DAEMON_TESTING.md`](./ISSUE_10_DAEMON_TESTING.md) - WatchService foundation
  - [`ISSUE_11_BASIC_LSP.md`](./ISSUE_11_BASIC_LSP.md) - LSP protocol and IPC
- **Related**:
  - `src/query.rs` - Query types and evaluation
  - `src/event.rs` - BeliefEvent definitions
  - `src/watch.rs` - WatchService orchestration
- **Roadmap Context**: v0.2.0+ feature for real-time collaboration
- **Future Work**:
  - ISSUE_16 (Future): Authorization and permission model for subscriptions
  - ISSUE_17 (Future): Websocket transport for web clients
  - Operational transform / CRDT for conflict-free editing

## Use Cases

### Use Case 1: Editor Extension (Real-Time Graph View)
- User opens a document in VSCode
- Extension subscribes to events for nodes in that document
- User edits file → parser updates → extension receives events → graph view updates
- Extension shows related documents (via relationships)

### Use Case 2: Dashboard (Project Overview)
- Dashboard subscribes to events for entire project (all networks)
- Shows live statistics: document count, node count, parse errors
- Updates in real-time as files change
- Uses websocket transport (future enhancement)

### Use Case 3: Collaborative Editing
- Multiple users editing different documents in same network
- Each client subscribes to events for their document + related nodes
- User A edits doc1.md → User B sees notification (doc1 changed)
- User B clicks link to doc1.md → sees updated content
- Bidirectional: User B adds link to doc2.md → User A sees new relationship

### Use Case 4: Build System Integration
- Build tool subscribes to events for specific file patterns (*.md, *.toml)
- File change → build tool receives event → triggers incremental rebuild
- Efficient: only rebuilds affected outputs
- Uses IPC transport for low latency

## Notes

### Relationship to Old "Focus" Concept

The old product-specific "focus" tracked:
- Awareness (set of BIDs user cares about)
- Attention (active documents)
- Radius (depth of relationship traversal)
- Motivation tracking (why user cares about these nodes)

This **reimagined focus** is:
- Query-based (more flexible than static BID lists)
- Event-driven (real-time updates, not polling)
- Bidirectional (clients can send events back)
- Transport-agnostic (LSP, websocket, IPC)
- Library feature (not product-specific)

### Performance Considerations

- **Event Routing**: O(1) for BID-indexed subscriptions, O(n) for complex queries
- **Memory**: Each subscription holds query + channel; estimate 1KB per subscription
- **Concurrency**: Use `Arc<RwLock<SubscriptionManager>>` for thread-safe access
- **Backpressure**: Bounded channels to prevent slow consumers from blocking

### Comparison to Other Systems

- **GraphQL Subscriptions**: Similar concept, but GraphQL-specific; we use noet queries
- **Firebase Realtime Database**: Query-based listeners; we add bidirectional and file integration
- **LSP Document Sync**: Similar pattern, but we extend beyond single documents to graph
- **WebSocket PubSub**: Redis-style topics; we use structured queries instead of topic strings

---

**Current Status**: Planning (v0.2.0+ roadmap)  
**Blockers**: ISSUE_10 (in progress), ISSUE_11 (planned)  
**Estimated Start**: After v0.1.0 release and ISSUE_11 completion
