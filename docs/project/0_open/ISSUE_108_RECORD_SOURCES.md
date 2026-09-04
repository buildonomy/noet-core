---
version = "0.1"
title = "Issue 108: Record Sources — Addressing R Evidence from the Graph"
---

# Issue 108: Record Sources — Addressing `R` Evidence from the Graph

**Priority**: MEDIUM
**Estimated Effort**: 4 days (RELATIVE COMPARISON ONLY)
**Dependencies**: Requires the record schema decision
(`docs/design/core/beliefbase_architecture.md` §4.3). Informed by Issue 103
(the range/identity pattern this generalizes). Supplies the addressing any
execution-observation consumer needs — Issue 18 among them — and unblocks
`living_corpus.md` §11's deferred question.

## Summary

Annotations cite `R` — as-run evidence — via `caused_by` and `evidence_hash`. But
nothing defines what a citation *points at*. "Test run 42" is not an address, and
noet has no way to resolve one, verify it, or say what it covers.

This issue defines a **`RecordSource`** interface: a codec-shaped abstraction over
log-like evidence stores that resolves a citation to a stable identity and a range
within that store, without ingesting the records themselves.

## The gap this closes

`living_corpus.md` §11 currently defers this:

> As-run records — test results, telemetry, build logs — are not in any of the
> three layers. noet is not a time-series store and should not become one.

That is the right scope boundary, but it leaves the citation dangling. An
attestation claiming "this requirement is verified, per runs 41–43" is only as
good as the ability to say *which* runs, in *which* store, and whether they still
exist. Without that, `evidence_hash` is a string nobody can check.

## What this is not

- **Not a time-series store.** noet does not hold `R`. It holds addresses of `R`.
- **Not an ingestion path.** A million telemetry samples produce zero nodes.
- **Not a query engine over logs.** Resolving a citation is not running an
  analysis; the owning store does that.

## Architecture

### Decision 1: No new `WeightKind`

`WeightKind` has exactly three variants, and they are not arbitrary — they are the
N/S/P content axes of `docs/essays/engineering_model_ontology.md`:

```rust
pub enum WeightKind {
    Section,   // Structural containment (S content)
    Pragmatic, // Procedural/operational relationships (P content)
    Epistemic, // Normative coupling and knowledge dependencies (N content)
}
```

`R` is emphatically **not** a fourth content axis. §6.1 is explicit: "`R` is not a
fourth spatial axis. `R` is the **time dimension** of model-spacetime." Adding
`WeightKind::Record` would encode as a content dimension the one thing the
ontology says is categorically not one, and would break the correspondence that
makes the three-kind model coherent — including the per-kind acyclicity invariant
and the stratified hashing that depends on it.

**A citation of evidence is an Epistemic edge.** "This claim draws from that
evidence" is exactly what Epistemic already means. The edge already has the right
type; what is missing is a well-formed thing on the other end of it.

### Decision 2: A reserved `Record` namespace, following the Asset precedent

noet has solved this shape before. Assets are content the compiler *cannot parse*
but *must address*: they get a reserved namespace (`UUID_NAMESPACE_ASSET`), a node
carrying a `content_hash` payload, and content-addressed handling — the bytes are
referenced and copied, never absorbed into the graph
(`src/codec/compiler.rs:5020-5095`).

`R` evidence is the same problem one level out: content noet cannot hold but must
address. Add `UUID_NAMESPACE_RECORD` alongside the existing three
(`src/properties.rs:90-110`) and a `Record` system network beside Href and Asset
(`beliefbase_architecture.md` §2.4).

**A record node is a reference, not a record.** One node per *cited span*, not per
observation:

| Field | Meaning |
|---|---|
| `source_id` | which store — resolves to a registered `RecordSource` |
| `range` | what span within it (see Decision 3) |
| `content_hash` | fingerprint of the cited span, for verification |
| `summary` | optional aggregate: count, pass rate, coverage, surprise distribution |

Citing 10,000 telemetry samples produces **one** node. This is the property that
keeps the graph from being flooded, and it is why the node addresses a *span*
rather than a record.

### Decision 3: `RecordRange` generalizes Issue 103's insight

Issue 103 establishes that a node in a source file has a **byte range** — an
address within a linear medium — and that identity plus range is what lets a graph
node point into content it does not contain. Log stores have the same structure
with a different coordinate system:

```rust
pub enum RecordRange {
    /// Byte or line span — log files, CI output
    Offset { start: u64, end: u64 },
    /// Time window — telemetry, metrics
    Temporal { from: Timestamp, to: Timestamp },
    /// Sequence span — event logs, replication logs, Kafka-like offsets
    Sequence { from: u64, to: u64 },
    /// Opaque store-defined selector — a query, a run ID, a test case ID
    Selector(String),
}
```

`Offset` is literally Issue 103's `Range<usize>` widened. The two should share the
resolution vocabulary where they can: "given an address, produce the content" and
"given content, produce its address" are the same two operations in both.

### Decision 4: `RecordSource` — a codec-shaped interface

The parallel to `DocCodec` is deliberate. `DocCodec` abstracts "a format noet can
parse into nodes." `RecordSource` abstracts "a store noet can address into
citations." Both are registered in a map, dispatched by a discriminator, and
implemented outside the core for domain-specific cases.

```rust
pub trait RecordSource {
    /// Stable identifier for this store; the `source_id` on record nodes.
    fn source_id(&self) -> &str;

    /// Does the cited span still exist, and does it still hash as recorded?
    fn verify(&self, range: &RecordRange, expected: &ContentHash)
        -> Result<Verification, BuildonomyError>;

    /// Aggregate the span without materializing it — count, pass rate,
    /// coverage, surprise distribution. This is the value `R` provides.
    fn summarize(&self, range: &RecordRange)
        -> Result<RecordSummary, BuildonomyError>;

    /// A human-followable link to the cited span.
    fn locate(&self, range: &RecordRange) -> Option<Url>;

    /// Optional: materialize the span. Bounded, opt-in, never called during parse.
    fn fetch(&self, range: &RecordRange, limit: usize)
        -> Result<Vec<RecordEntry>, BuildonomyError> { Err(Unsupported) }
}
```

`summarize` is the load-bearing method. §3.4 of the ontology: "statistics on `R`
across the operating domain … constitute the model's credibility evidence." The
power is in the aggregate, so the interface's primary operation returns an
aggregate. `fetch` is deliberately last, defaulted to unsupported, and never on
the parse path — the moment it becomes routine, noet has drifted into being a log
store.

`verify` is what makes a citation checkable. Three outcomes worth distinguishing:
`Valid`, `Missing` (the store no longer has it), and `Changed` (it has it, but the
hash differs — the most alarming case, and the one that silently passes today).

### Decision 5: Registration, not compilation

A `RecordSource` is configured, not discovered. Registration declares
`source_id` → implementation + connection details, in the same shape as codec
registration. Built-in implementations should be few and generic:

- **File-backed log** — offset/line ranges over a text file
- **Directory of run artifacts** — selector by run ID

Anything domain-specific (a test-results database, a telemetry service, a CI API)
is an out-of-tree implementation, exactly as domain codecs are. noet-core ships
the interface and the trivial cases.

### Decision 6: A record node may only exist as cited evidence

Record nodes are **never free-standing**. One may enter the graph only as the
sink of an Epistemic citation from an annotation.

The reason is the conduit model in `docs/design/annotation/living_corpus.md` §5: a Pragmatic
edge is a *declared conduit* awaiting an actor (`S(P)`), and an annotation is the
`R` proving an actor traversed it. Evidence is meaningful because some accountable
party *cited* it in a claim. Evidence with no citing claim is a log entry, and a
graph that accumulates those is a log index — precisely the outcome the
claim/evidence split exists to prevent.

Operationally:

- Creating a record node is a side effect of resolving an annotation's citation,
  never a standalone ingestion step. There is no "import this log into the graph"
  operation, and adding one would be a design regression.
- A record node whose last citing annotation is superseded becomes garbage. It is
  not an error — the evidence still exists in its store — but the graph has no
  further reason to address it. Collection policy is out of scope; the invariant
  is not.
- `check_consistency` should report a free-standing record node as a defect, on
  the same footing as an unresolved cross-reference.

This is the property that keeps the mechanism honest: `RecordSource` makes
evidence *addressable*, and this decision keeps it from making evidence
*accumulable*.

### How this composes with annotations

```
                    ┌─ Epistemic edge (draws from)
  Annotation ───────┤
  "verified per     │   Record node (Record namespace)
   runs 41-43"      └──   source_id: "ci-runs"
                          range: Sequence { from: 41, to: 43 }
                          content_hash: <of that span>
                          summary: { count: 3, passed: 3 }
                                 │
                                 │  RecordSource::verify / summarize
                                 ▼
                          the actual store — outside noet
```

The annotation's `caused_by` / `evidence_hash` resolve to the record node; the record
node resolves through its `RecordSource` to the store. Nothing about the graph
grows with the volume of evidence.

## Implementation Steps

1. **Namespace and node shape** (0.5 days)
   - [ ] `UUID_NAMESPACE_RECORD` in `src/properties.rs` beside the existing three
   - [ ] `Record` system network alongside Href and Asset; document in
         `beliefbase_architecture.md` §2.4
   - [ ] Node payload schema: `source_id`, `range`, `content_hash`, `summary`

2. **`RecordRange` and `RecordSource`** (1 day)
   - [ ] The two types above, with serde round-trip tests
   - [ ] `RecordSourceMap` registry mirroring `CodecMap`'s shape
   - [ ] Document the `DocCodec` parallel so the analogy is discoverable

3. **Reference implementations** (1 day)
   - [ ] File-backed log source — `Offset` ranges, real `verify` and `summarize`
   - [ ] Directory-of-artifacts source — `Selector` ranges
   - [ ] Both generic; nothing domain-specific in-tree

4. **Citation resolution** (1 day)
   - [ ] Resolve an annotation's `caused_by` / `evidence_hash` to record nodes
   - [ ] Epistemic edge from annotation to record node — the citation is
         provenance ("draws from"), not action; see `living_corpus.md` §5
   - [ ] Record nodes are created only by citation resolution; no standalone
         ingestion path exists (Decision 6)
   - [ ] Surface `Missing` / `Changed` through `check_consistency` — a broken or
         altered evidence citation is a consistency defect, and this is the point
         of the issue
   - [ ] Surface a free-standing record node (no citing annotation) as a defect

5. **Tests** (0.5 days)
   - [ ] Citing a 10,000-entry span produces exactly one node
   - [ ] No API path creates a record node without a citing annotation
   - [ ] `verify` returns `Changed` when the underlying span is mutated
   - [ ] `verify` returns `Missing` when it is deleted; `check_consistency` reports it
   - [ ] `summarize` aggregates without materializing (assert `fetch` is not called)
   - [ ] An unregistered `source_id` degrades to an unresolved citation with a
         diagnostic, never a panic

## Testing Requirements

- Graph node count is invariant to evidence volume: 10 entries and 10,000 entries
  in one cited span both yield one record node.
- A corpus with no `RecordSource` registered parses unchanged; record nodes are
  simply unresolved, exactly as an unresolved cross-reference is today.
- `summarize` is never called during parse — evidence stores may be remote and slow.

## Success Criteria

- [ ] `WeightKind` is unchanged; evidence citations are Epistemic edges
- [ ] A record node addresses a *span* and carries an aggregate, never per-entry data
- [ ] `RecordSource` is registered like a codec and implementable out-of-tree
- [ ] An annotation citing evidence resolves to a verifiable address
- [ ] A changed or deleted cited span is surfaced by `check_consistency`
- [ ] Execution observations can be addressed without a consumer inventing its
      own mechanism

## Risks

- **Scope creep into being a log store.** `fetch` is the thin end of the wedge.
  → **Mitigation**: defaulted to `Unsupported`, bounded by `limit`, never on the
  parse path; `summarize` is the primary operation and the tests assert it.
- **Remote sources block compilation.** A `RecordSource` backed by a network
  service could stall a parse. → **Mitigation**: resolution never happens during
  parse; verification is an explicit `check_consistency` operation.
- **`RecordRange` variants proliferate.** Every store wants its own coordinate
  system. → **Mitigation**: `Selector(String)` is the escape hatch; resist adding
  variants until two real sources need the same one.
- **The Asset precedent is followed too literally.** Assets are copied into the
  output; evidence must not be. → **Mitigation**: no `fetch` in the export path;
  record nodes carry addresses only.

## Open Questions

- **Does a record node get a `version` in the annotation-anchor sense?** Records
  are immutable, so their content hash is stable by definition — which suggests
  the anchor question does not arise. But a *span* can grow (runs 41–43 becomes
  41–50). Recommend: the cited span is fixed at citation time; a wider span is a
  new node and a new citation.
- **Where does `summary` come from — the source, or a cached computation?**
  Calling `summarize` on every query is too expensive for a remote store; caching
  it makes the graph carry derived data that can go stale. Recommend caching in
  the node payload with the `content_hash` as the validity key.
- **Does this subsume the replication log?** `federated_belief_network.md` §2.4
  establishes that a Layer 2 replication log entry is `R`. It could plausibly be
  addressed by a `RecordSource`, making replication throughput and lag queryable
  the same way test coverage is. Interesting, not required; do not design for it
  until something asks.
- **Relationship to Issue 103's range vocabulary.** `Offset` duplicates
  `Range<usize>`. Should they share a type, or does the difference in medium
  (source file vs. log store) justify separate ones? Recommend separate types with
  a documented parallel — a source range is resolvable by the compiler, a record
  range only by an external store.

## References

- `docs/essays/engineering_model_ontology.md` §3.4 (`R` as-run records, statistics
  as credibility evidence), §6.1 (`R` is the time dimension, not a fourth axis),
  §9.3 (credibility as a typed floor map — the aggregate form `summary` serves)
- `docs/design/annotation/living_corpus.md` §2 — annotations as a privileged subset of `R`
  records are evidence — the distinction this issue implements
- `docs/design/annotation/living_corpus.md` §11 — the deferred "where does `R` live" question
  this issue answers
- `src/properties.rs:90-110` — `UUID_NAMESPACE_*` constants; the reserved-namespace
  precedent
- `src/properties.rs:745` — `WeightKind`, and why it stays at three
- `src/codec/compiler.rs:5020-5095` — asset content-hash handling; the
  address-don't-absorb precedent this generalizes
- `docs/design/core/beliefbase_architecture.md` §2.4 — system network namespaces
- `docs/design/core/beliefbase_architecture.md` §3.6 — `DocCodec`, the interface shape
  `RecordSource` mirrors
- Issue 103 — node source ranges; the identity+range pattern generalized here
- Issue 18 — an aspirational stub; whatever it becomes will cite execution
  observations through this interface rather than defining its own
