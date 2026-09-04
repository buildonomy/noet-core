---
title = "Federated Belief Network: Sharing Annotations, Source, and Compiled State"
authors = "Andrew Lyjak, Claude Sonnet 4.6"
last_updated = "2025-07-02"
status = "Draft"
version = "0.1"
---

# Federated Belief Network

## 1. Purpose

How several people or machines share a corpus. The three layers
(`living_corpus.md` §2) have different answers — one is solved by existing tools,
one is derived and needs nothing, and one is the real problem. §1.1 makes that
split; §1.2 describes the sharing model it implies.

§§2–7 specify the Layer 2 case: multiple `DbConnection` + `DocumentCompiler`
pairs — each owning a distinct subset of the belief graph — coordinating to form
a single queryable knowledge space. Each node is responsible only for the
networks instantiated under its compiler's `repo()` root, but can read (and
optionally subscribe to) content from peer nodes.

> [!NOTE]
> **Intended trajectory (recorded, not yet reflected below).** This document is
> to become the **general use case** for a sidecar-propagation design owned by
> [`collaboration_overlay.md`](./collaboration_overlay.md): federating a corpus
> is the general case of propagating a held-out annotation overlay across a scope
> boundary. §1.2 below (scoped queues and percolation) is the propagation
> mechanism and stays here as the authoritative description of it; the overlay
> being propagated is specified in `living_corpus.md` §2 ("Layer 3's live
> projection is a held-out BeliefBase").
>
> **Nothing below has been restructured for this.** Recorded so the direction is
> not lost.

> [!IMPORTANT]
> **Status: design sketch, not a specification.** No part of this document is
> implemented, and no issue currently owns Layer 3 federation. Treat it as the
> starting point for whoever picks that up — the reasoning is sound but the
> shapes are provisional, and an implementing issue should expect to revise them
> and then reconcile this document to what was built.
>
> **Reframing: federation is primarily a Layer 3 protocol.** This document was
> written as a Layer 2 design — replicating the compiled graph between peers —
> and §§3–7 still read that way. That is the smaller half of the problem, and
> arguably the optional one. See §1.1.
>
> The three-layer model (source / belief graph / annotation) is specified in
> [`living_corpus.md`](./living_corpus.md) §2, which took ownership of it from
> §3.7 of this document.

### 1.1 What Actually Needs Federating

The three layers have very different sharing requirements, and only one of them
needs a protocol this project has to invent.

| Layer | Shareable by | Status |
|---|---|---|
| **L1 Source** | git — clone, branch, PR, merge | **Solved.** Use it. |
| **L2 Graph** | recompiling from shared source | **Derived.** Replication is an optimization. |
| **L3 Annotation** | nothing that exists | **The actual problem.** |

**Layer 2 is a pure function of Layer 1.** Two peers with the same source produce
the same graph. Replicating the compiled graph is therefore replicating a *cache*
— worth doing when a peer cannot or should not compile the source itself (no
access, no toolchain, prohibitive size), but never the thing that makes a
federation possible. §§3.2–3.6 specify that optimization and remain valid for it.

**Layer 1 sharing is a solved problem and should be left solved.** Source is text
in a repository; the ecosystem for sharing it — forks, branches, pull requests,
CI — is mature and every user already has it. A source-distribution protocol
invented here would be a worse git. Recommended patterns:

- A shared corpus repository with per-contributor forks and pull requests.
- CI that compiles on merge and publishes shards, so consumers who only read get
  Layer 2 without compiling (this is the Layer 2 "replication" case, served by
  static hosting rather than a peer protocol).
- Submodules or subtree merges where several corpora compose into one.

**Layer 3 cannot be derived from anything.** An annotation is authored, exists
nowhere else, and has no ecosystem. If two people are to share what they have
reviewed, flagged, or drafted, something must move those records between them.
That is the protocol this document should specify.

It is also the easy one, which is why the framing matters: annotation records are
immutable with globally unique IDs, so merge is **set union** and conflicts are
impossible by construction (`living_corpus.md` §2). A Layer 3 federation needs
transport and policy, not a consistency model.

### 1.2 Scoped Queues and Percolation

The unit of Layer 3 federation is a **scope** — a record set with a boundary.
Issue 105 already defines three on a single machine (repo / user / shared) with
union semantics across them. Federation generalizes that: a peer's scope is
another in the same union.

What makes this more than a sync protocol is that **runs nest**
(`living_corpus.md` §5, Issue 109). A `RunStart` may cite a parent `run_id`, so a
team-level effort can decompose into individual work queues:

```
shared queue        RunStart A  "hazard review, section 3"
  │
  ├─ alice's queue   RunStart B  parent=A  task=<fault-tree step>
  │                    ...dozens of working records, drafts, dead ends...
  │                  RunEnd   B  result: ...
  │
  └─ bob's queue     RunStart C  parent=A  task=<peer-review step>
                       ...
                     RunEnd   C
```

Alice's intermediate records are hers. What crosses the boundary into the shared
queue is the **`RunEnd` summary** — the folded result of the child run — plus any
source edits it produced. The working records stay local unless someone asks for
them.

This is **percolation**: a federation boundary that transmits folded outcomes
rather than raw record sets. It is possible precisely because a run is already a
bracketed, foldable unit with a declared relationship to its parent's template —
the boundary has something principled to fold *to*. Three consequences worth
stating:

- **It bounds volume.** Full replication of every peer's annotation log does not
  scale and mostly transmits noise. Percolation moves what the parent asked for.
- **It gives privacy a natural shape.** "My drafts are mine until I finish" is a
  scope boundary, not an access-control feature bolted on.
- **It preserves auditability.** The summary cites its constituents by
  `EventId`; the chain is traversable *if* the detail scope is available. An
  unresolvable citation is the ordinary partial-scope case
  (Issue 109 §Failure Modes), not corruption.

> **This open question may already be answered.** Issue 110's layered
> read-through model makes policy-vs-transport a false dichotomy: **filtering on
> read is what a layer stack does**, and percolation is the decision about which
> layer a record is *written* to. Read composition and write placement are
> orthogonal, so both halves hold without conflict. Under that model percolation
> is **layer promotion** — the same operation as Issue 105's flush, which was
> already suspected to be one mechanism. Confirm when 110 lands.

**Open**: whether percolation is a *policy* over a general sync mechanism (peers
exchange everything; scopes filter on read) or a *transport-level* filter
(summaries are all that is transmitted). The first is simpler and keeps the G-Set
intact end to end; the second is what actually bounds volume and enables privacy.
Probably both, at different boundaries. Unresolved, and it is the central design
question for Layer 3 federation.

> **Status of §§2–7 below.** They specify Layer 2 replication: peer identity, the
> event log, watermarks, pull-based sync, and the federated query layer. That
> material is sound for the Layer 2 optimization case, and much of it — `PeerId`,
> watermarks, resumable pull — transfers directly to Layer 3. But it is written
> as though Layer 2 were the point. Rewriting it around §1.1 is follow-up work,
> not done here.

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

This is the same shape as the Layer 3 annotation log. The connection is not coincidental:
both are replicated state machines where:

1. Each actor appends to a local log it owns.
2. Peers pull (or are pushed) new entries from each other's logs.
3. Derivative indices are rebuilt from the canonical log on demand.

Point 3 is the **durable store plus live projection** pattern that recurs at every layer
(`living_corpus.md` §3): shards hydrate into the in-memory graph, annotation records fold
into derived state, and here a replicated log rebuilds a SQLite replica. Recognising it as
one pattern is what keeps three loaders from being written.

**Where the two logs differ** is the consistency model, and the difference is not
incidental:

| | Layer 2 belief log (this doc) | Layer 3 annotation log |
|---|---|---|
| Entries | graph mutations — **order-dependent** | claims — **order-independent** |
| Ownership | exactly one writer per node | many writers, no coordination |
| Merge | replay in `(actor, sequence)` order | set union (G-Set) |
| Conflicts | impossible by partitioned ownership | impossible by immutability |

A rename followed by a content update is not the same as the reverse, so Layer 2 entries
must be *replayed in order*. Layer 3 records are immutable and uniquely identified, so they
merge by union and order affects only presentation. Both reach "no conflicts" — by
different routes — and neither requires a CRDT library.

Note the relationship to Issue 16 (Automerge), which is deferred and optional:
neither log is a specialisation of the other. Both are specialisations of the
store-plus-projection pattern.

### 2.4. This Log Is `R`, Not Annotation

A replication log entry is an **as-run record** (`R` in
`../essays/engineering_model_ontology.md` §3.4) — "a read of `s_t`", an observation of what
this peer's pipeline did.

So is an annotation. Both are `R`; §3.4 is explicit that "an automated test pipeline is as
much a `P`-identity as a human reviewer." The distinction is not categorical but one of
**subject and volume**: an annotation observes a node in *this* graph and arrives at
human scale, so it can be stored and projected individually; a replication entry observes
this peer's pipeline and arrives at machine scale, so it cannot
(`living_corpus.md` §2).

The distinction matters for three concrete reasons, and note that all three are consequences
of volume and subject rather than of kind:

1. **Volume.** Log entries are machine-generated, one per graph mutation, unbounded. Layer 3
   annotation records are human-generated and bounded by deliberate acts. Routing replication
   entries into the annotation store would swamp it.
2. **Projection.** Each annotation projects into the graph as a node plus edges
   (`attestation_fabric.md` §12.3). A log entry must *not* — it is already a mutation *of*
   the graph. Projecting it would be circular.
3. **Value model.** §3.4 of the ontology: the epistemic power of `R` is in *accumulation and
   statistics*, not in the individual record. Replication throughput, lag distributions,
   and per-peer divergence are the questions worth asking of this log — none of which are
   answered by looking at a single entry.

**The two connect by citation.** An attestation is a claim that cites `R` as its evidence —
"this graph state is verified, per these runs" — using the provenance chain in
`attestation_fabric.md` §4.2a. So the belief log can *become* evidence backing a Layer 3
claim, without any of its entries being annotations.

What the two logs share is the **envelope** (§3.2) — identity, ordering, causality — not the
payload semantics. `Event::Belief` and `Event::Annotation` are siblings in the payload enum
precisely so that one transport and one ordering discipline can carry both without
conflating them.

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

A `PeerId` is one kind of `ActorId` (§3.2). The envelope's `actor` field is deliberately
opaque so that the same type identifies a replicating peer here, a human attester at
Layer 3, and a CI pipeline at either. Nothing in the codebase defines peer identity yet —
`PeerId` is greenfield, which is why the envelope's shape was free to be chosen to serve
both layers.

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

The event log entry adds a small replication envelope around the existing `BeliefEvent`.

> [!IMPORTANT]
> **Do not define an envelope here.** Two failures are available and both have
> been tried: a federation-specific `LogEntry` is a competing record schema, and
> copying the annotation envelope is the same mistake inverted — that envelope is
> scoped to annotations (`beliefbase_architecture.md` §4.3), and `BeliefEvent`s
> do not carry one.
>
> What Layer 2 replication needs is a **replication frame**: a peer identity, a
> per-peer sequence number, and a payload. Whether that reuses the annotation
> envelope's `EventId`/`ActorId` types or defines its own is unresolved and is
> Layer 2 work — nothing depends on it today, since §1.1 establishes that Layer 2
> replication is an optimization rather than the point of federation.
>
> Two constraints hold whatever shape it takes:
>
> - **A `PeerId` is one kind of `ActorId`.** If the types are shared, they are
>   shared in that direction; the annotation layer must not learn about peers.
> - **`EventOrigin` stays on `BeliefEvent`.** It answers "already applied to my
>   state?" — a local dispatch concern that survives replication unchanged. A
>   replicated entry is `EventOrigin::Remote`; its producing peer is separate
>   information carried by the frame.
>
> A vector clock (`causal_deps: BTreeMap<PeerId, u64>`) is **not** required for a
> read-only replica — per-peer sequence ordering suffices. Any timestamp in the
> frame is display-only.

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
successfully applied. The same `peer_watermarks` table is the cursor model the Layer 3 sync
peer uses for its `/events?since=<cursor>` endpoint (`collaboration_overlay.md`), so the two
replication paths share a resumption mechanism even though they do not share a consistency
model:

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

1. Each `Envelope` carries its `actor` (§3.2). The local `DbConnection` accepts `INSERT OR
   REPLACE` for entries whose `actor` matches the local node's `PeerId`; for peer entries it
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

> [!IMPORTANT]
> **Awaiting revision against the synchronous overlay model (Issue 110).**
>
> Issue 110 recommends an `OverlayGraph` — a read-through wrapper implementing
> `BeliefSource` over a base graph plus annotation layers. **That is the same
> construction as `FederatedBeliefSource`**, differing only in whether layers are
> distinguished by *owner* (peer) or by *scope* (repo / user / shared). §1.2
> already says a peer's scope is another member of the same union, so there is
> one layer stack, not two mechanisms.
>
> Two things below will change:
>
> - **Whole-node dedup → field-level, per-layer.** The fan-out below dedupes
>   "by `Bid`", with local winning. An overlay carries *field-level* patches, so
>   the unified resolution is per-field-by-layer. "Local wins" becomes an
>   ordering fact — local sits above peers in the stack — rather than a rule.
> - **`async` must not be the default.** These methods are `async` because peers
>   are remote and may be unreachable. Local overlay layers are synchronous.
>   Availability should be a *layer property*, not a property of the composition
>   that forces `async` onto every local read.
>
> **Issue 110 settles the synchronous case first and owns opening the follow-on
> issue that revises this document.** The distributed case has constraints the
> local case does not; designing both at once would let the harder one distort
> the simpler one.

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

### 3.7. Position in the Layer Model

> The three-layer model is specified in
> [`living_corpus.md`](./living_corpus.md) §2. What follows is only what is
> specific to Layer 2 replication.

This document specifies **Layer 2 replication**: how the compiled belief graph is shared
between peers. It is one arrow in the larger picture:

| Concern | Owned by |
|---|---|
| Three-layer model, PII surfaces, annotate → promote loop | `living_corpus.md` |
| Record and envelope schema | `beliefbase_architecture.md` §4.3 |
| Layer 3 annotation store and its sync | `living_corpus.md` §3, `collaboration_overlay.md` |
| **Layer 2 peer replication** | **this document** |

The Layer 2 / Layer 3 consistency comparison that used to live here is now in §2.3, beside
the CRDT discussion it belongs to.

**Inter-layer flow.** A Layer 1 → 2 parse may produce a Layer 3 record (for instance, a
CI-emitted assertion about a node that just changed). Layer 3 flows back to Layer 1 by
redline promotion — `living_corpus.md` §7 and Issue 106 — which is a *source edit*, not a
replication event, and therefore re-enters this document's scope only as the next parse.

Federation composes with Layer 3 rather than subsuming it. A peer that replicates another's
belief graph does not thereby receive its annotations: those propagate by record union over
the annotation store, whose sync peer may be an entirely different process
(`collaboration_overlay.md`). Two nodes can share a graph and disagree about what has been
reviewed — correctly, since review is per-actor.

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

3. **Access control**: Keyhive is the target authorization layer for both this document's
   peer subscriptions and Layer 3 credentials (`attestation_fabric.md` §7.2). Initially,
   peer subscriptions are unauthenticated (trusted network assumed). Keyhive is pre-release;
   neither layer should take a hard dependency on it.

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
| `BeliefEvent` | In-process channel message | Becomes an `Envelope` payload as `Event::Belief(..)` |
| `EventOrigin` | Local/Remote apply flag | Unchanged — orthogonal to `Envelope.actor` (§3.2) |
| `commit_generation` | Local idle signal | Local sequence number in vector clock |
| `compiler_idle` | Debouncer hold-off | Per-peer "source stable" signal |
| `Transaction` | Batch DB commit | Batch commit + log append |

The principle is: the existing components are correct and do not need to change. The
federated layer wraps and extends them rather than replacing them.

---

## 7. Open Questions

1. **Log storage format**: Should the event log be stored in the same SQLite DB as the
   belief graph (separate table) or in a separate SQLite file? SQLite is simplest. Automerge
   is no longer a candidate here: Layer 2 entries are order-dependent (§2.3), which is the
   case CRDT merge does not serve.

1a. **Does Layer 2 replication share transport with Layer 3 sync?** Both are
   "pull entries since a cursor" over HTTP. Sharing the transport would avoid two
   protocols; keeping them separate preserves the property that a peer can replicate a graph
   without receiving annotations, and vice versa. Recommend separate endpoints on a shared
   envelope format — but this is unresolved and should be settled before either is built.

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