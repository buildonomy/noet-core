---
title = "Federated Belief Network: Distributed Compiler-DB Coordination"
authors = "Andrew Lyjak, Claude Sonnet 4.6"
last_updated = "2025-07-02"
status = "Draft"
version = "0.1"
---

# Federated Belief Network

## 1. Purpose

This document describes an architectural extension to the noet-core compilation pipeline in
which multiple `DbConnection` + `DocumentCompiler` pairs — each owning a distinct subset of
the belief graph — coordinate to form a single queryable knowledge space. Each node is
responsible only for the networks instantiated under its compiler's `repo()` root, but can
read (and optionally subscribe to) content from peer nodes.

This is the natural generalisation of the current single-node model:

```
Current (single node):

  filesystem
      ↓
  DocumentCompiler  →  DbConnection (SQLite)
                            ↓
                        application queries

Federated (multiple nodes):

  filesystem A          filesystem B          filesystem C
      ↓                     ↓                     ↓
  Compiler A            Compiler B            Compiler C
      ↓                     ↓                     ↓
  DbConnection A  ←──→  DbConnection B  ←──→  DbConnection C
                              ↓
                      unified query surface
```

The motivating use cases are:

- A team where each member runs a local noet server owning their personal network, but can
  traverse links into peers' networks for read access.
- A CI/documentation server that ingests multiple source repositories as separate networks
  and serves a combined query API.
- An offline-first mobile client that owns a lightweight subset of a larger shared graph and
  syncs incrementally when connectivity is available.

---

## 2. Core Concepts

### 2.1. Network Ownership

Every belief node belongs to exactly one **home network**, identified by the compiler's
`repo()` path. The home compiler is the **authority** for that node: it parses the source
files, resolves references, emits `BeliefEvent`s, and writes the canonical mtime records.

A node can *reference* nodes in a peer network (via cross-network `NodeKey::Path` or
`NodeKey::Id` references), but cannot write to them. Writes always flow through the owning
compiler.

### 2.2. DbConnection as Replication Target

`DbConnection` is currently a local SQLite pool. In the federated model it becomes a
**replication target**: a node's DB contains the full, authoritative state for its owned
networks *plus* a materialized read-replica of any peer content it has subscribed to.

The replica is explicitly marked (via a `peer_id` column on replicated tables) so queries
can distinguish owned from replicated content and so conflict resolution is never needed:
owned content always wins.

### 2.3. Coordination as a CRDT Problem

The `commit_generation` counter introduced in `WatchService::wait_for_idle` (Issue 51) is
a degenerate monotonic log: a single sequence number advancing as the local pipeline commits
work. Generalised across peers, this becomes a **vector clock** — one sequence number per
peer — and the "is there work to do?" question becomes "is my position behind any peer's
tail?"

This is the same problem Automerge (Issue 16) solves for activity events. The connection is
not coincidental: both the compilation pipeline and the activity event log are replicated
state machines where:

1. Each actor appends to a local log it owns.
2. Peers pull (or are pushed) new entries from each other's logs.
3. Derivative indices (SQLite) are rebuilt from the canonical log on demand.

The federated belief network is therefore a specialisation of the Issue 16 event sourcing
architecture applied to the *parse output* (BeliefEvents) rather than user activity events.

---

## 3. Architecture

### 3.1. Node Identity

Each node in the federation is identified by a `PeerId` — a stable UUID assigned at first
startup and persisted in `config.toml`. A `PeerId` maps to:

- A network root path (local filesystem, for owned networks).
- A transport endpoint (URL or socket address, for remote peers).
- A set of owned network `Bid`s (advertised during handshake).

```rust
pub struct PeerId(Uuid);

pub struct PeerRecord {
    pub id: PeerId,
    pub endpoint: Option<String>,   // None = local (in-process)
    pub owned_networks: Vec<Bid>,   // Advertised at handshake; cached locally
}
```

### 3.2. The Belief Event Log

`BeliefEvent` (currently an in-process `tokio::sync::mpsc` channel) becomes the
fundamental unit of replication. Each emitted event is appended to a **local event log**
before being committed to SQLite:

```
DocumentCompiler
    │
    │  BeliefEvent stream
    ▼
EventLog (append-only, owned by this node)
    │
    ├──→  local DbConnection  (immediate, synchronous commit)
    │
    └──→  peer DbConnections  (async push/pull, best-effort)
```

The event log entry adds a small replication envelope around the existing `BeliefEvent`:

```rust
pub struct LogEntry {
    /// Monotonically increasing within this peer's log.
    pub sequence: u64,
    /// Wall-clock time (for display; not used for ordering).
    pub timestamp: SystemTime,
    /// Which peer produced this entry.
    pub peer_id: PeerId,
    /// The belief graph mutation.
    pub event: BeliefEvent,
    /// Sequence numbers of entries this entry causally depends on,
    /// keyed by peer_id. Empty for entries with no cross-peer dependencies.
    pub causal_deps: BTreeMap<PeerId, u64>,
}
```

The `causal_deps` field is the start of a vector clock. For the initial implementation it
can be omitted (empty map) — total ordering by `(peer_id, sequence)` is sufficient for
read-only replication.

### 3.3. Replication Protocol

Replication is **pull-based** at the protocol level (simpler to implement, easier to reason
about back-pressure) with an optional push notification to reduce latency:

```
Peer A                              Peer B
  │                                   │
  │── SUBSCRIBE(peer_id=A, from=42) ──→│  "send me entries from seq 42 onwards"
  │                                   │
  │←─ ENTRIES([42..55]) ──────────────│
  │                                   │
  │  (B appends new entry 56)         │
  │←─ NOTIFY(peer_id=B, head=56) ────│  optional push hint
  │                                   │
  │── PULL(peer_id=B, from=56) ──────→│
  │←─ ENTRIES([56]) ─────────────────│
```

Each `DbConnection` tracks its **watermark** per peer — the highest sequence number it has
successfully applied:

```sql
CREATE TABLE peer_watermarks (
    peer_id   TEXT NOT NULL,
    watermark INTEGER NOT NULL DEFAULT 0,
    PRIMARY KEY (peer_id)
);
```

On reconnect, a node resumes from its stored watermark. This makes replication
**idempotent** and **resumable** without requiring the sender to retain a full log forever
(entries below all peers' watermarks can be compacted).

### 3.4. Ownership and Conflict Avoidance

A core invariant: **only the owning compiler writes to a node**. This is enforced by:

1. Each `LogEntry` carries its `peer_id`. The local `DbConnection` accepts `INSERT OR
   REPLACE` for entries whose `peer_id` matches the local node's ID; for peer entries it
   uses `INSERT OR IGNORE` on the primary key (no overwrites of replicated content).
2. The compiler's `DocumentCompiler` only parses paths under its `repo()` root. Cross-peer
   references are resolved at query time via the federated query layer (§3.6), not at parse
   time.
3. The `WatchService` file watcher only watches paths under the local `repo()` root.

This means there are **no merge conflicts** in the belief graph: each node in the graph has
exactly one owner, and only that owner's compiler can modify it.

### 3.5. Subscription and Dependency Tracking

A node subscribes to a peer when it encounters an unresolved cross-network reference during
parsing. The compiler's existing `UnresolvedReference` diagnostic becomes the trigger:

```
Compiler parses doc_a.md which links to peer://network-B/some-node
    │
    ↓
GraphBuilder::push_relation detects cross-peer reference
    │
    ↓
FederationManager::ensure_subscribed(peer_id=B, network_bid=...)
    │
    ├── already subscribed? → no-op
    └── new? → open replication channel, pull from watermark 0
```

Once subscribed, the local DB contains a replica of the relevant entries from peer B. The
reference can then be resolved against the local replica rather than making a synchronous
remote call during compilation.

This is **eventual consistency**: cross-peer references may be unresolved on the first
parse pass and resolved on a subsequent pass once replication has caught up. The existing
multi-pass reparse logic in `DocumentCompiler` already handles this gracefully.

### 3.6. Federated Query Layer

The current `BeliefSource` trait is the query interface. In the federated model, a
`FederatedBeliefSource` wraps multiple `DbConnection`s and fans out queries:

```rust
pub struct FederatedBeliefSource {
    /// Local DB — authoritative for owned networks.
    local: DbConnection,
    /// Replicated peer DBs — read-only views of peer content.
    peers: Vec<(PeerId, DbConnection)>,
}

impl BeliefSource for FederatedBeliefSource {
    async fn get_states(&self, query: &Query) -> Result<...> {
        // Fan out to all DBs, merge results, deduplicate by Bid.
        // For conflicts (same Bid from multiple sources), local wins.
    }
}
```

For most queries the fan-out is transparent to the caller. The `peer_id` provenance is
available for display purposes (e.g. "this node is owned by peer B") but not required for
correctness.

### 3.7. Relation to Issue 16 (Automerge Event Log)

Issue 16 describes an **activity event log** — user actions, procedure matches, redlines —
using Automerge as the CRDT substrate. The federated belief network describes a
**compilation output log** — parsed graph mutations — using a simpler pull-based replication
protocol.

These are complementary layers of the same stack:

```
Layer 3: Activity Events (Issue 16)
  Automerge CRDT, multi-producer, append-only
  "What did users and systems do?"

Layer 2: Belief Graph (this document)
  Pull-based replication, single-owner-per-node
  "What does the document graph contain?"

Layer 1: Source Files (current implementation)
  Filesystem, WatchService, DocumentCompiler
  "What do the files say?"
```

Layer 2 feeds Layer 3: a `FileParsed` event (Layer 1→2) may trigger an `ActivityEvent`
(Layer 2→3) such as `node_updated` or `reference_resolved`. Layer 3 can also flow back to
Layer 1 via procedure execution writing new source files.

The two layers intentionally use different consistency models:

| | Belief Graph (Layer 2) | Activity Events (Layer 3) |
|---|---|---|
| Ownership | Single owner per node | Multi-producer |
| Conflict model | No conflicts (partitioned ownership) | CRDT merge |
| Substrate | SQLite + pull replication | Automerge |
| Ordering | `(peer_id, sequence)` total order | Lamport clock |
| Compaction | Watermark-based log truncation | Rotation by time/count |

Automerge is **not** the right substrate for Layer 2: belief graph mutations are
non-commutative (a node rename followed by a content update is not the same as the reverse),
and the single-owner invariant means CRDT merge is never needed. Automerge's strengths
(multi-writer, commutative ops, offline merge) are only needed at Layer 3.

---

## 4. Local Pipeline Implications

The federated model clarifies the semantics of the **existing** `commit_generation` counter
in `WatchService`:

- It is the **local sequence number** for the local peer's belief event log.
- `wait_for_idle` waiting for `commit_generation > snapshot` is equivalent to: "wait until
  the local log has advanced past the point where I took my snapshot."
- In a federated system, a caller might instead wait for `commit_generation > snapshot` on
  *all* relevant peers — a generalisation that does not require changing the current API,
  only adding a peer-aware variant.

The `compiler_idle: AtomicBool` flag introduced for debouncer hold-off maps cleanly onto
a per-peer "is this source currently being compiled?" signal that remote subscribers could
also observe to know when a peer's output is stable enough to pull.

---

## 5. Extension Points

The following integration points are explicitly left open for implementation:

1. **Transport**: The replication protocol is transport-agnostic. Initial implementation
   can use in-process channels (`tokio::sync::mpsc`) for testing; HTTP/2 or WebSocket for
   LAN/WAN peers; local UNIX socket for same-machine multi-process setups.

2. **Log compaction**: Entries below all peers' watermarks can be dropped. Compaction
   policy (how long to retain, whether to snapshot) is left to the implementation.

3. **Access control**: Keyhive (Issue 16, Phase 2) is the target authorization layer.
   Initially, peer subscriptions are unauthenticated (trusted network assumed).

4. **Network discovery**: Peers are currently configured explicitly in `config.toml`.
   mDNS or a DHT-based discovery mechanism is out of scope for v1.

5. **Conflict resolution for renames**: If two peers rename the same node concurrently
   (which should be impossible under single-owner semantics but could happen due to bugs or
   manual DB edits), last-writer-wins on `sequence` is the fallback.

---

## 6. Relationship to Existing Components

| Component | Current role | Federated role |
|---|---|---|
| `DbConnection` | Local SQLite pool | Local authority + peer replica store |
| `BeliefSource` trait | Query interface | Extended by `FederatedBeliefSource` |
| `DocumentCompiler` | Single-repo parser | Unchanged — still single-repo |
| `WatchService` | Local file watcher + pipeline | Unchanged — still local |
| `BeliefEvent` | In-process channel message | Becomes `LogEntry` payload |
| `commit_generation` | Local idle signal | Local sequence number in vector clock |
| `compiler_idle` | Debouncer hold-off | Per-peer "source stable" signal |
| `Transaction` | Batch DB commit | Batch commit + log append |

The principle is: the existing components are correct and do not need to change. The
federated layer wraps and extends them rather than replacing them.

---

## 7. Open Questions

1. **Log storage format**: Should the event log be stored in the same SQLite DB as the
   belief graph (separate table), in a separate SQLite file, or in Automerge files? SQLite
   is simplest; Automerge would unify with Issue 16 but adds complexity at Layer 2.

2. **Subscription granularity**: Should nodes subscribe at the network level (all events
   for network B) or at the node level (only events touching specific `Bid`s)? Network-level
   is simpler; node-level reduces bandwidth for large peer networks.

3. **First-pass unresolved references**: The current compiler emits a warning for unresolved
   cross-network references. In the federated model, should it silently defer (subscribe and
   re-queue) instead? This would make cross-peer references transparent to document authors.

4. **`wait_for_idle` across peers**: Should `WatchService::wait_for_idle` optionally block
   until peer watermarks have also advanced? This is useful for integration tests that span
   multiple nodes.

5. **Relation to `BeliefBase::is_balanced`**: The existing balance check verifies internal
   graph consistency for a single DB. In a federated context, "balanced" may need to account
   for known-pending peer entries (entries in the log but not yet applied to the replica).