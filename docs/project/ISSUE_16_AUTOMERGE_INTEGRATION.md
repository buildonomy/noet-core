# Issue 16: Distributed Event Log with Automerge + Keyhive

**Priority**: MEDIUM (Post-v0.1.0)  
**Estimated Effort**: 3-4 weeks  
**Dependencies**: ISSUE_10 (WatchService), ISSUE_15 (Filtered Event Streaming)  
**Context**: Part of v0.4.0+ roadmap - Event sourcing architecture for distributed activity tracking

## Summary

Implement a **distributed event log system** using Automerge CRDT for append-only activity streams from multiple producers (users, devices, applications, bots). Events are merged chronologically with logical clocks, stored in rotated Automerge files optimized for cloud archival, and indexed in SQLite for efficient querying. Keyhive provides distributed authorization based on focus (query filters define what each user can read/write).

**Key Principle**: This is **event sourcing**, not state sync. The canonical log is append-only, immutable events. Derivative indices enable application-specific queries.

## Goals

1. Implement append-only distributed event log with multiple producers
2. Chronological merging using Lamport clocks (total ordering)
3. Store events in rotated Automerge files (optimized for cloud archival)
4. Build derivative SQLite indices for efficient queries
5. Focus-based authorization via Keyhive (user:focus → read/write permissions)
6. Integration with WatchService and filtered event streaming (ISSUE_15)
7. Support procedure matching and redline learning (see procedure_engine.md)

## Architecture

### Event Sourcing Model

```
Multiple Activity Producers:
  - User A's laptop → ActivityEvent stream
  - User A's phone → ActivityEvent stream
  - User B's desktop → ActivityEvent stream
  - Application X → ActivityEvent stream
  - Bot/Sensor Y → ActivityEvent stream

         ↓ (append to Automerge log)

    Canonical Event Log (Automerge)
    - Rotated files: events_2025-01.automerge, events_2025-02.automerge
    - Append-only, immutable
    - Chronologically ordered (Lamport clock)

         ↓ (build derivative indices)

    Query Indices (SQLite)
    - by_user: user_id → [event_ids]
    - by_bid: bid → [event_ids]
    - by_time_window: time_bucket → [event_ids]
    - by_event_type: event_type → [event_ids]

         ↓ (application-specific views)

    Event Streams (ISSUE_15)
    - Filtered by query (focus)
    - Authorized by Keyhive capabilities
```

### Event Schema

```rust
/// Universal activity event (append-only)
pub struct ActivityEvent {
    // Identity
    id: EventId,              // Globally unique: (device_id, sequence_number)
    
    // Ordering
    timestamp: Timestamp,     // Wall clock (for human display, may have drift)
    lamport_clock: u64,       // Logical clock (for total ordering)
    
    // Provenance
    device_id: DeviceId,      // Which device produced this
    user_id: UserId,          // Which user
    producer: String,         // Which system (e.g., "DwellingPointVisit", "WatchService")
    
    // Semantics
    event_type: String,       // "action_detected", "subscribed", "viewed", "edited", etc.
    target: Option<Bid>,      // What node/resource this affects
    
    // Payload
    payload: serde_json::Value, // Event-specific data (varies by event_type)
    
    // Authorization context
    focus_context: Option<FocusId>, // Which focus/query scope this relates to
}

// Total ordering:
// ORDER BY lamport_clock ASC, timestamp ASC, id ASC
```

**Examples:**

```rust
// User subscribed to query (ISSUE_15)
ActivityEvent {
    id: "laptop-1:seq-1234",
    timestamp: "2025-01-23T10:30:00Z",
    lamport_clock: 5678,
    device_id: "laptop-1",
    user_id: "alice",
    producer: "WatchService",
    event_type: "subscribed",
    target: Some(bid!("project-docs")),
    payload: json!({
        "query": { /* PaginatedQuery */ },
        "motivation": "Working on ISSUE_11"
    }),
    focus_context: Some("focus-work-lsp"),
}

// Procedure engine detected action (procedure_engine.md)
ActivityEvent {
    id: "phone-1:seq-456",
    timestamp: "2025-01-23T07:30:00Z",
    lamport_clock: 5679,
    device_id: "phone-1",
    user_id: "alice",
    producer: "ActionInferenceEngine",
    event_type: "action_detected",
    target: Some(bid!("act_morning_routine")),
    payload: json!({
        "inference_id": "morning_v2",
        "confidence": 0.92,
        "duration_minutes": 45,
        "supporting_data": [/* ObservationEvents */]
    }),
    focus_context: Some("focus-daily-routine"),
}

// Redline correction (redline_system.md)
ActivityEvent {
    id: "laptop-1:seq-1235",
    timestamp: "2025-01-23T10:35:00Z",
    lamport_clock: 5680,
    device_id: "laptop-1",
    user_id: "alice",
    producer: "ParticipantFeedback",
    event_type: "procedure_correction",
    target: Some(bid!("act_morning_routine")),
    payload: json!({
        "match_id": "match-789",
        "correction_type": "PartialMatch",
        "participant_note": "I skipped shower today"
    }),
    focus_context: Some("focus-daily-routine"),
}
```

### Automerge Storage: Rotated Logs

**Key Design Decision**: Optimize for cloud storage and archival, not real-time sync.

```
event_logs/
  ├── events_2025-01.automerge  (archived, read-only)
  ├── events_2025-02.automerge  (archived, read-only)
  ├── events_2025-03.automerge  (current, append-only)
  └── metadata.json             (rotation config, focus indices)
```

**Rotation Strategy:**

```rust
pub struct EventLogConfig {
    // Rotation policy
    rotation_period: RotationPeriod,  // Monthly, Weekly, Daily
    max_events_per_file: usize,       // e.g., 10000
    
    // Cloud sync
    sync_to_cloud: bool,
    cloud_provider: Option<String>,   // "S3", "GCS", "Azure", etc.
    
    // Retention
    archive_after_days: u32,          // Move to cold storage
    delete_after_days: Option<u32>,   // Permanent deletion (GDPR compliance)
}

pub enum RotationPeriod {
    Daily,
    Weekly,
    Monthly,
    ByEventCount(usize),
}
```

**Benefits:**
- ✅ Small files suitable for cloud upload/download
- ✅ Archived files are immutable (can verify integrity)
- ✅ Only current file needs to be in memory
- ✅ Easy to implement retention policies (delete old files)
- ✅ Efficient focus-based loading (load only relevant time periods)

**Automerge File Structure:**

```rust
// Each rotated file is an Automerge document
{
    "metadata": {
        "start_time": "2025-03-01T00:00:00Z",
        "end_time": "2025-03-31T23:59:59Z",
        "event_count": 8543,
        "users": ["alice", "bob"],
        "focus_contexts": ["focus-work-lsp", "focus-daily-routine"],
    },
    "events": [
        // Automerge list (CRDT)
        { /* ActivityEvent 1 */ },
        { /* ActivityEvent 2 */ },
        // ...
    ],
    "lamport_clock": 999999,  // Highest clock in this file
}
```

### SQLite Derivative Indices

**Purpose**: Fast queries for application-specific views. Rebuilt from Automerge on startup or when focus changes.

```sql
-- Main event index (materialized view of Automerge)
CREATE TABLE events (
    id TEXT PRIMARY KEY,
    timestamp INTEGER NOT NULL,
    lamport_clock INTEGER NOT NULL,
    device_id TEXT NOT NULL,
    user_id TEXT NOT NULL,
    producer TEXT NOT NULL,
    event_type TEXT NOT NULL,
    target_bid TEXT,
    payload JSON,
    focus_context TEXT,
    source_file TEXT NOT NULL  -- Which Automerge file this came from
);

-- Derivative indices (optimized for queries)
CREATE INDEX idx_lamport ON events(lamport_clock, timestamp, id);
CREATE INDEX idx_user ON events(user_id, lamport_clock);
CREATE INDEX idx_bid ON events(target_bid, lamport_clock);
CREATE INDEX idx_type ON events(event_type, lamport_clock);
CREATE INDEX idx_time ON events(timestamp);
CREATE INDEX idx_focus ON events(focus_context, lamport_clock);

-- Focus-specific caching (what events does this focus care about?)
CREATE TABLE focus_event_cache (
    focus_id TEXT NOT NULL,
    event_id TEXT NOT NULL,
    relevance_score REAL,  -- How relevant (1.0 = exact match, 0.0 = barely relevant)
    cached_at INTEGER NOT NULL,
    PRIMARY KEY (focus_id, event_id)
);

-- Track which Automerge files are loaded
CREATE TABLE loaded_logs (
    file_name TEXT PRIMARY KEY,
    loaded_at INTEGER NOT NULL,
    event_count INTEGER NOT NULL,
    min_lamport INTEGER NOT NULL,
    max_lamport INTEGER NOT NULL
);
```

**Rebuilding Strategy:**

```rust
pub async fn rebuild_indices(
    focus_id: &FocusId,
    config: &EventLogConfig,
    db: &DbConnection,
) -> Result<(), BuildonomyError> {
    // 1. Determine which Automerge files are relevant to this focus
    let relevant_files = determine_relevant_files(focus_id, config)?;
    
    // 2. Load Automerge files and extract events
    for file_path in relevant_files {
        let doc = load_automerge_file(&file_path)?;
        let events = extract_events_from_doc(&doc)?;
        
        // 3. Filter events by focus query
        for event in events {
            if focus_matches_event(focus_id, &event)? {
                // Insert into SQLite
                insert_event(&db, &event, &file_path)?;
                
                // Update focus cache
                update_focus_cache(&db, focus_id, &event)?;
            }
        }
        
        // 4. Mark file as loaded
        mark_file_loaded(&db, &file_path, events.len())?;
    }
    
    Ok(())
}
```

### Focus-Based Authorization (Keyhive)

**Authorization Model**: A **focus** (from ISSUE_15) defines both:
1. **Query scope**: What events/nodes the user cares about
2. **Permissions**: What operations are allowed within that scope

```rust
pub struct Focus {
    id: FocusId,
    user_id: UserId,
    query: Query,  // From ISSUE_15 (PaginatedQuery)
    
    // Keyhive capabilities
    capabilities: Vec<Capability>,
}

// Keyhive capability structure
pub struct Capability {
    // What can be done
    action: CapabilityAction,  // "read", "write", "append"
    
    // Within what scope
    scope: CapabilityScope,
    
    // With what constraints
    constraints: Vec<Constraint>,
}

pub enum CapabilityAction {
    Read,           // Can query events
    Write,          // Can modify events (rare, mostly append)
    Append,         // Can add new events
    Subscribe,      // Can create filtered streams (ISSUE_15)
}

pub enum CapabilityScope {
    AllEvents,                          // Everything (admin)
    UserEvents(UserId),                 // Only events by this user
    FocusEvents(FocusId),               // Events matching this focus query
    EventType(String),                  // Only specific event types
    TargetBid(Bid),                     // Events affecting this node
}

pub struct Constraint {
    constraint_type: String,
    value: serde_json::Value,
}

// Example constraints:
// - time_window: {"start": "2025-01-01", "end": "2025-12-31"}
// - rate_limit: {"max_events_per_hour": 1000}
// - approval_required: {"approver": "user_bob"}
```

**Authorization Check:**

```rust
impl EventLog {
    /// Append event with authorization
    pub async fn append_event(
        &mut self,
        event: ActivityEvent,
        user_capabilities: &[Capability],
    ) -> Result<(), BuildonomyError> {
        // 1. Check if user has "Append" capability for this event
        let allowed = user_capabilities.iter().any(|cap| {
            cap.action == CapabilityAction::Append &&
            cap.scope.matches_event(&event) &&
            cap.constraints.iter().all(|c| c.check(&event))
        });
        
        if !allowed {
            return Err(BuildonomyError::Unauthorized(format!(
                "User {} cannot append event type {} to focus {}",
                event.user_id, event.event_type, event.focus_context.unwrap_or_default()
            )));
        }
        
        // 2. Assign Lamport clock
        let lamport = self.increment_lamport_clock();
        let mut event = event;
        event.lamport_clock = lamport;
        
        // 3. Append to current Automerge file
        self.append_to_current_log(event)?;
        
        // 4. Check if rotation needed
        if self.should_rotate()? {
            self.rotate_log().await?;
        }
        
        Ok(())
    }
    
    /// Query events with authorization
    pub async fn query_events(
        &self,
        query: &Query,
        user_capabilities: &[Capability],
    ) -> Result<Vec<ActivityEvent>, BuildonomyError> {
        // 1. Check if user has "Read" capability for this query scope
        let allowed = user_capabilities.iter().any(|cap| {
            cap.action == CapabilityAction::Read &&
            cap.scope.matches_query(query)
        });
        
        if !allowed {
            return Err(BuildonomyError::Unauthorized);
        }
        
        // 2. Query SQLite indices
        let events = self.db.query_events(query).await?;
        
        // 3. Filter by capability constraints
        let filtered = events.into_iter()
            .filter(|e| user_capabilities.iter().any(|cap| cap.allows_read(e)))
            .collect();
        
        Ok(filtered)
    }
}
```

**Example Focus with Capabilities:**

```rust
// User Alice working on LSP implementation
Focus {
    id: "focus-work-lsp",
    user_id: "alice",
    query: Query {
        // All events related to ISSUE_11 or LSP-related BIDs
        expression: Expression::Or(vec![
            Expression::BidMatch(bid!("ISSUE_11")),
            Expression::PathMatch("/docs/project/ISSUE_11_*.md"),
            Expression::Metadata("tags", vec!["lsp", "language-server"]),
        ]),
    },
    capabilities: vec![
        // Can read all events in this focus
        Capability {
            action: CapabilityAction::Read,
            scope: CapabilityScope::FocusEvents("focus-work-lsp"),
            constraints: vec![],
        },
        // Can append events (subscriptions, corrections, etc.)
        Capability {
            action: CapabilityAction::Append,
            scope: CapabilityScope::FocusEvents("focus-work-lsp"),
            constraints: vec![
                Constraint {
                    constraint_type: "rate_limit".to_string(),
                    value: json!({"max_events_per_hour": 1000}),
                },
            ],
        },
        // Can subscribe to filtered streams
        Capability {
            action: CapabilityAction::Subscribe,
            scope: CapabilityScope::FocusEvents("focus-work-lsp"),
            constraints: vec![],
        },
    ],
}
```

## Implementation Steps

### Phase 1: Event Log Foundation (1 week)

1. **Define Event Schema** (1 day)
   - [ ] Create `ActivityEvent` struct with all fields
   - [ ] Implement Lamport clock logic (increment, merge)
   - [ ] Define event types (subscribed, action_detected, procedure_correction, etc.)
   - [ ] Write serialization/deserialization tests
   - [ ] Document event schema with examples

2. **Automerge Log Storage** (2 days)
   - [ ] Implement `EventLog` struct wrapping Automerge document
   - [ ] Implement append-only operations (no updates/deletes)
   - [ ] Add rotation logic (by time, by event count)
   - [ ] File naming convention: `events_YYYY-MM.automerge`
   - [ ] Test rotation and file creation

3. **SQLite Derivative Indices** (2 days)
   - [ ] Create SQLite schema (events, focus_event_cache, loaded_logs)
   - [ ] Implement `rebuild_indices()` from Automerge files
   - [ ] Add focus-specific filtering during rebuild
   - [ ] Test query performance with large event sets
   - [ ] Implement incremental updates (append to SQLite when append to Automerge)

4. **Integration with WatchService** (2 days)
   - [ ] Add `EventLog` to `WatchService` struct
   - [ ] Generate `ActivityEvent`s from:
     - Subscriptions (ISSUE_15)
     - File modifications
     - Query executions
   - [ ] Test event generation and logging
   - [ ] Verify Lamport clock monotonicity across restarts

### Phase 2: Authorization with Keyhive (1 week)

5. **Focus-Based Authorization Model** (2 days)
   - [ ] Define `Capability`, `CapabilityAction`, `CapabilityScope` types
   - [ ] Implement `Focus` struct with query + capabilities
   - [ ] Add authorization checks to `append_event()` and `query_events()`
   - [ ] Test authorization rules (allow, deny, constraints)

6. **Keyhive Integration** (3 days)
   - [ ] Add `keyhive` crate dependency (when stable)
   - [ ] Implement capability distribution (how users get capabilities)
   - [ ] Add capability verification before event operations
   - [ ] Test distributed authorization scenarios
   - [ ] Document Keyhive setup and configuration

7. **Focus Management API** (2 days)
   - [ ] Implement `create_focus()`, `update_focus()`, `delete_focus()`
   - [ ] Store focus definitions (SQLite or Automerge?)
   - [ ] Add focus-based log loading (`load_logs_for_focus()`)
   - [ ] Test focus lifecycle and cache invalidation

### Phase 3: Cloud Sync and Archival (1 week)

8. **Cloud Upload/Download** (3 days)
   - [ ] Implement S3/GCS/Azure blob upload for rotated files
   - [ ] Add download on-demand (when querying old events)
   - [ ] Implement integrity checks (hash verification)
   - [ ] Test upload/download with large files
   - [ ] Add retry logic for network failures

9. **Retention Policies** (2 days)
   - [ ] Implement archive policy (move to cold storage after N days)
   - [ ] Implement deletion policy (GDPR compliance)
   - [ ] Add tombstone markers for deleted events
   - [ ] Test retention enforcement
   - [ ] Document retention configuration

10. **Performance Optimization** (2 days)
    - [ ] Benchmark Automerge file loading
    - [ ] Optimize SQLite queries (EXPLAIN QUERY PLAN)
    - [ ] Implement caching (LRU cache for recent events)
    - [ ] Add metrics (events/sec, query latency, storage usage)

### Phase 4: Integration with Procedure Engine (1 week)

11. **Action Detection Events** (2 days)
    - [ ] Integrate with ActionInferenceEngine (action_inference_engine.md)
    - [ ] Generate `action_detected` events with inference metadata
    - [ ] Store supporting ObservationEvents in payload
    - [ ] Test end-to-end: observation → inference → event log

12. **Redline Correction Events** (2 days)
    - [ ] Integrate with Redline System (redline_system.md)
    - [ ] Generate `procedure_correction` events from participant feedback
    - [ ] Link corrections to original procedure matches
    - [ ] Update learned_parameters table from correction events

13. **Procedure Matching Queries** (3 days)
    - [ ] Implement temporal queries (events in time window)
    - [ ] Implement causality queries (events leading to outcome)
    - [ ] Add procedure pattern matching over event sequences
    - [ ] Test matching against templates with variations

## Testing Requirements

**Unit Tests:**
- `ActivityEvent` serialization/deserialization
- Lamport clock increment and merge logic
- Automerge append-only operations
- SQLite query correctness
- Authorization checks (allow/deny)
- Rotation logic (time-based, count-based)

**Integration Tests:**
- Multiple producers append events → merged chronologically
- Events persisted across restarts (Automerge durability)
- SQLite indices rebuilt from Automerge
- Focus-based filtering works correctly
- Authorization prevents unauthorized access
- Cloud upload/download roundtrip

**Performance Tests:**
- Append latency (target: < 10ms per event)
- Query latency (target: < 100ms for recent events)
- Rotation time (target: < 1 second)
- Memory usage with large logs (target: < 500MB for current log)
- SQLite index size (target: < 10% of Automerge file size)

## Success Criteria

- [ ] Multiple producers can append events concurrently
- [ ] Events merged chronologically with Lamport clocks
- [ ] Rotated Automerge files stored and archived
- [ ] SQLite indices provide fast queries
- [ ] Focus-based authorization works (Keyhive)
- [ ] Cloud sync uploads/downloads rotated files
- [ ] Integration with WatchService (ISSUE_10)
- [ ] Integration with filtered event streaming (ISSUE_15)
- [ ] Integration with procedure engine (action detection, corrections)
- [ ] Retention policies enforced (archival, deletion)
- [ ] Documentation complete with examples

## Risks

**Risk**: Automerge file size grows unbounded  
**Mitigation**: Rotation policy (monthly files, max 10k events); cloud archival; focus-based loading

**Risk**: SQLite rebuild from Automerge is slow  
**Mitigation**: Incremental updates; only rebuild on startup or focus change; cache loaded files

**Risk**: Keyhive authorization too complex  
**Mitigation**: Start simple (user:focus → read/append); defer fine-grained constraints to v0.5.0

**Risk**: Lamport clock doesn't capture causality  
**Mitigation**: For procedure matching, use bounded causality (application-specific); vector clocks if needed later

**Risk**: Cloud sync costs too high  
**Mitigation**: Compress Automerge files; use cold storage; configurable retention; user pays model

## Open Questions

1. **Storage format**: Store capabilities in Automerge or SQLite?
   - Leaning: SQLite (easier to query, focus definitions are device-specific)

2. **Sync protocol**: How do devices exchange Lamport clock state?
   - Approach: Include highest clock in file metadata; merge on load

3. **Focus lifecycle**: Who creates/manages focus definitions?
   - Approach: User creates via UI; system suggests based on usage patterns

4. **Event schema versioning**: How to handle schema changes over time?
   - Approach: Include schema_version in ActivityEvent; migration on load

5. **Bounded causality**: How much causal context to include in events?
   - Approach: Event can reference "caused_by" event_id; applications traverse as needed

## Decision Log

**Decision 1: Rotated Automerge Files (Not Single Doc)**
- Date: 2025-01-23
- Rationale: Optimize for cloud storage; small files easier to upload/download; immutable archives
- Impact: More complex loading logic; need to track which files are loaded
- Alternative rejected: Single growing Automerge doc (too large for cloud sync)

**Decision 2: SQLite for Derivative Indices (Not Automerge Queries)**
- Date: 2025-01-23
- Rationale: SQL is faster for complex queries; can rebuild from Automerge if corrupted
- Impact: Dual storage (Automerge canonical, SQLite cache); need to keep in sync
- Alternative rejected: Query Automerge directly (too slow for complex filters)

**Decision 3: Focus-Based Authorization (Not Per-Event)**
- Date: 2025-01-23
- Rationale: Focus already exists (ISSUE_15); natural permission boundary; user mental model
- Impact: Need to evaluate if event matches focus query for auth checks
- Alternative rejected: Per-event ACLs (too granular, hard to manage)

**Decision 4: Lamport Clocks Sufficient (Not Vector Clocks)**
- Date: 2025-01-23
- Rationale: Total ordering sufficient for most use cases; applications can track bounded causality
- Impact: Can't capture full causality; procedure matching needs application logic
- Alternative rejected: Vector clocks (more complex, larger metadata)

**Decision 5: Keyhive for Authorization (Not Custom)**
- Date: 2025-01-23
- Rationale: Distributed auth is hard; Keyhive is battle-tested; capability-based model fits
- Impact: Dependency on pre-release library; need to monitor stability
- Deferred: Full integration to v0.5.0+ when Keyhive stable; mock for v0.4.0

## References

- **Depends On**:
  - [`ISSUE_10_DAEMON_TESTING.md`](./ISSUE_10_DAEMON_TESTING.md) - WatchService foundation
  - [`ISSUE_15_FILTERED_EVENT_STREAMING.md`](./ISSUE_15_FILTERED_EVENT_STREAMING.md) - Focus and subscriptions
- **Related**:
  - `procedure_engine.md` - Procedure matching and redline learning
  - `action_inference_engine.md` - Action detection events
  - `redline_system.md` - Correction feedback loop
- **External**:
  - [Automerge](https://automerge.org/) - CRDT library
  - [Keyhive](https://github.com/inkandswitch/keyhive) - Distributed authorization (pre-release)
  - Event Sourcing: Martin Fowler, Greg Young
- **Roadmap Context**: v0.4.0 feature for event sourcing architecture

## Use Cases

### Use Case 1: Multi-Device Activity Tracking
- User works on laptop, generates `action_detected` events
- Later opens tablet, queries recent events via focus
- SQLite indices provide fast lookup
- User sees chronological activity history across devices

### Use Case 2: Procedure Matching with Redlines
- ActionInferenceEngine generates `action_detected` events
- Redline system queries event sequences to match procedures
- Participant provides corrections → `procedure_correction` events
- Learned parameters updated from correction event log

### Use Case 3: Collaborative Project Tracking
- Multiple users working on same project (shared focus)
- Each appends events (subscriptions, file edits, comments)
- Keyhive ensures users can only write to authorized focus
- Query "all events for ISSUE_11" shows merged activity from all users

### Use Case 4: Cloud Archival and Compliance
- Events rotated monthly to Automerge files
- Files uploaded to S3 cold storage
- Retention policy: delete after 2 years (GDPR compliance)
- User can still query old events (download on-demand)

### Use Case 5: Real-Time Collaboration (ISSUE_15 Integration)
- User subscribes to filtered event stream (focus-work-lsp)
- New events appended by other users
- Event log routes to subscriber via ISSUE_15 mechanism
- Bidirectional: subscriber can append corrections back to log

---

**Current Status**: Planning (v0.4.0+ roadmap)  
**Blockers**: ISSUE_10 (in progress), ISSUE_15 (planned)  
**Estimated Start**: After v0.3.0 release  
**Note**: Keyhive integration deferred to v0.5.0+ (waiting for stable release); mock authorization for v0.4.0