---
title = "Codec Determinism Contract"
authors = "Andrew Lyjak, Claude"
last_updated = "2026-09-09"
status = "Draft"
version = "0.1"
dependencies = ["beliefbase_architecture.md", "content_versioning.md"]
---

# Codec Determinism Contract

## 1. Purpose

A codec is the boundary where bytes become nodes and edges. Everything
downstream — content hashes, annotation anchors, incremental parse, shard
export, codec-regression detection — assumes that boundary is a **function**:
the same bytes produce the same graph.

This document states the guarantees a `DocCodec` implementation must uphold, and
names what breaks when each is violated. It is the contract behind
`content_versioning.md`; that document explains what hashes *mean*, this one
explains why they are computable at all.

> **Read this before implementing a codec, and before changing how an existing
> one emits nodes or edges.** The trait signature does not enforce any of the
> guarantees below — a codec can violate all of them and still compile, still
> parse, and still produce a plausible-looking graph. The failures are silent
> and appear one subsystem away.

## 2. The guarantees

### G1 — Node output is a pure function of content

`DocCodec::parse` receives `content: &str` and must produce the same `IRNode`
set for the same input. No wall clock, no ambient environment, no random
identifiers, no iteration over a hash map whose order is not fixed.

**Consumes it**: `content_hash` determinism (`content_versioning.md` §6), which
in turn is every annotation anchor. An unstable hash stales every annotation on
every restart — indistinguishable from the whole-corpus failure mode the anchor
design exists to avoid.

Two known hazards, both recorded in `content_versioning.md` §6:

- `toml::Table` is `BTreeMap`-backed under the current lockfile and iterates in
  sorted key order. Enabling the `preserve_order` feature anywhere in the
  dependency graph switches it to insertion order and silently destabilizes
  every hash.
- `payload["text"]` is regenerated from the event stream. Any nondeterminism in
  that path is inherited by the hash, which is why the stability test must cover
  the full parse → DB → export → hydrate round trip rather than repeated hashing
  of one in-memory node.

### G2 — Node output is a pure function of *this file's* content

Not merely deterministic, but deterministic **from one file read in isolation**.
A codec must not let the output for file A depend on what it observed while
parsing file B.

**Consumes it**: incremental parse. Skip logic re-parses a file whose hash
changed and reuses cached nodes otherwise; if A's nodes depend on B, editing B
silently leaves A stale in the graph while its hash says clean. That is graph
corruption with no error path.

Cross-file *references* are fine and expected — they resolve later, through
`cache_fetch` and the unresolved-reference diagnostic. The prohibition is on
cross-file **content dependence** during `parse`.

### G3 — Edge emission order is a function of the source text

Edges are created as the parser reads, one thread per file, so the order in
which a codec emits them is determined by the order of the constructs in the
source. `WEIGHT_SORT_KEY` is then assigned as `max(existing) + 1`
(`BeliefBase::assign_sort_key`, `src/beliefbase/base.rs:2228`), and within a
batch `sort_relation_events` (`src/paths/pathmap.rs:340`) reorders
topologically rather than by arrival time — so the property survives batching.

**Consumes it**: the anchor hash design. `content_versioning.md` §4.3 excludes
sibling order from the hash *because* sort keys are derived from source text
that `content_hash` already covers — including them would hash one fact twice.
That argument is only valid while G3 holds.

> **This is the guarantee most likely to be broken by a plausible future
> change.** Concurrent cross-file edge resolution, an externally supplied edge
> stream, or a codec that emits edges from a parallel iterator would each break
> it — not by producing wrong edges, but by making sort keys a parse artifact
> rather than a property of the document. If that happens,
> `content_versioning.md` §4.3 must be revisited before the change lands, not
> after. A codec that cannot uphold G3 should say so explicitly rather than
> appearing to satisfy it.

### G4 — Identity is derived, never minted, wherever it must survive a rebuild

A codec that assigns identity must derive it from stable input. The v5-over-a-
normalized-string pattern is the house style: `Bid::codec_namespace` normalizes
its term with `to_anchor` before hashing, so casing and separator differences
cannot fork a namespace. See `identity/identity_derivation.md` §3.

**Consumes it**: everything anchored to a BID. A minted identity that nobody
persists re-mints on the next cold parse.

### G5 — Output is comparable across binaries, not merely across runs

Nothing binary-specific may enter node content: no pointer values, no
allocation-order-dependent iteration, no build timestamps, no compiler version.

**Consumes it**: codec-regression detection (`content_versioning.md` §7.2). The
codec registry is a public extension surface (`CODECS.insert_codec`,
`WALK_CODECS.register`, `CLAIM_MAP.claim`), and the `DocCodec` trait is small
enough that *signature* breaks are caught by the compiler. The undetected class
is behavioural: the codec still compiles, still runs, and emits subtly different
nodes. A stored hash manifest is what detects that, and it only works if the
hash is stable between binaries.

### G6 — Diagnostics go to the diagnostics channel

Author-visible problems — duplicate anchors, unresolvable references,
malformed directives — belong in the `diagnostics: &mut Vec<ParseDiagnostic>`
parameter, not in `tracing` output and not in node content.

**Consumes it**: `last_diagnostics` and the parse-observation model. A
diagnostic embedded in node content changes the content hash, so a warning
would stale every annotation on the node it warns about.

## 3. What the trait does not enforce

All six guarantees are conventions. The compiler checks none of them, and a
violating codec produces output that looks correct in every single-process test.
The cheapest checks available:

| Guarantee | Test that catches a violation |
|---|---|
| G1 | Hash the same corpus in two processes; compare |
| G2 | Parse A+B, then B+A; compare A's nodes |
| G3 | Parse twice, compare `WEIGHT_SORT_KEY` on every edge |
| G4 | Cold-parse twice with no cache; compare BIDs of nodes carrying stable ids |
| G5 | Hash manifest over a fixture corpus, compared across builds |
| G6 | Assert the diagnostics vec is non-empty for a known-bad fixture |

The round-trip form matters for G1 and G4: hashing one in-memory node twice
proves nothing, because the instability lives in serialization and re-hydration.

## 4. References

| Document | Relationship |
|---|---|
| `core/beliefbase_architecture.md` §3.2, §3.6 | Codec dispatch, the two registries, `DocCodec` as the frontend interface |
| `identity/content_versioning.md` §4.3, §6, §7.2 | What the hashes mean; the determinism requirement; the regression detector |
| `identity/identity_derivation.md` | Minted vs. derived identity; the reserved namespaces |
| `project/LESSONS_LEARNED.md` | The failure modes these guarantees prevent |
