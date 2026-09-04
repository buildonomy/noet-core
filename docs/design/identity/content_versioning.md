---
title = "Content Versioning: Scoped Identity for Nodes and Claims"
authors = "Andrew Lyjak, Claude"
last_updated = "2026-09-01"
status = "Draft"
version = "0.1"
dependencies = ["query_model.md", "dag_model.md", "living_corpus.md"]
---

# Content Versioning

> [!NOTE]
> **Target architecture.** Nothing specified here is implemented. §9 maps each
> element to the issue that will build it. Supersedes the hashing material in
> `../project/trades/superseded/event_record_unification.md`, which reached a
> `hash(n, kind, radius)` family but not the general form in §4.

## 1. Purpose

A compiled corpus needs to answer: **has the thing I was looking at changed?**

The question is harder than it sounds, because "the thing I was looking at" is
rarely one node. A reviewer who proofreads a paragraph looked at that paragraph.
A reviewer who approves a section looked at everything under it. An engineer who
verifies a requirement looked at the evidence chain behind it — which may not
include the requirement's own text at all.

These are different scopes, and they must stale independently. If a corpus can
only answer "did this node's bytes change", every claim about it inherits the
wrong sensitivity: the section review misses a child edit, and the verification
misses everything.

This document defines **scoped content identity** — how a version is computed
over an arbitrary scope, how the common scopes are cached, and what consumes
them.

**Scope.** This document owns version computation. It does not own the annotation
record schema (`beliefbase_architecture.md` §4.3), the annotation
model (`living_corpus.md`), or the query algebra it builds on
(`query_model.md`).

---

## 2. What a Version Is Not

Two rejected definitions, recorded because both were in circulation and both are
locally reasonable.

**Not a build identifier.** An earlier design used `asset_version` — an FNV-1a
hash over the entire compiled beliefbase — as the anchor for review sign-offs.
The argument was that coarseness is a feature: cross-references mean any change
could affect any interpretation, so re-approval prompts are the safe default.

That does not survive a realistic corpus. Every build changes a whole-corpus
hash, so every sign-off is permanently stale, and a signal that always fires
carries no information. It trains reviewers to ignore it — the opposite of the
intent. Superseded; see `collaboration_overlay.md` §3.2.

**Not the node's equality relation.** `BeliefNode` implements `PartialEq` over
`bid`, `kind`, `title`, `schema`, `payload`, `id`, **and `metadata`**
(`src/properties.rs:1091`). Equality answers "is this the same node state?" —
merges and `compute_diff` need `metadata` propagated. A version answers "is this
the same node *content*?" and must exclude it, because `metadata` carries git
commit status, layout coordinates, and content profiles, all of which change
without the node's meaning changing. Reusing the `PartialEq` field set would
stale every annotation on every commit.

The two field sets are deliberately almost-identical and must not be unified.

---

## 3. Selecting a Scope

An annotation does not merely *have* an anchor. It **selects which scope it
asserts against**, and that selection is a semantic property of the annotation
kind rather than a storage detail.

| Claim | Scope | Anchor |
|---|---|---|
| "I proofread this paragraph" | the node's own fields | `content_hash` |
| "I reviewed this section" | Section containment | `section_hash` |
| "I verified this requirement" | the reasoning chain behind it | `epistemic_hash` |
| "I signed off on this and its direct dependencies" | one hop | `QuerySpec`, depth 1 |
| "I reviewed all Class-A items under §3" | filtered subtree | `QuerySpec` with a filter |

(The named hashes are the cached scopes; §5 defines them.)

Two annotations on the same node, by the same actor, at the same instant, can
legitimately stale differently — because they claimed different things.

**`protocol_id` carries the selection.** The protocol registry
(`attestation_fabric.md` §6) already resolves a `protocol_id` to a record schema
and a `graph_roles` block. Scope selection belongs in the same entry: a
`noet:signoff:v1` record anchors to `section_hash`, a `noet:attest:v1` to the
Epistemic closure, and a custom `local:<team>:<name>:v1` to whatever its
definition names — including an arbitrary query.

The last two rows are why no fixed set of precomputed hashes suffices. Neither is
reachable by any `(edge kind, closure)` pair, and both are ordinary review
practice. Whatever a version *is*, it has to be defined over a scope the corpus
did not anticipate.

---

## 4. The Model: `(QuerySpec, tape_hash)`

A version anchor is a **scope** plus a **fingerprint of what that scope
contained**:

> **`(QuerySpec, tape_hash)`** — the query defining the scope, paired with a hash
> over the serialized query and the content hashes of the tape it returned.

The `QuerySpec` says *what was looked at*. The `tape_hash` says *what it
contained at the time*. Staleness is: re-run the query, re-hash, compare.

This is the general form because a scope is exactly what `query_model.md` already
knows how to express — a seed plus a projection chain. Nothing new is needed to
say "this node", "this node and its Section descendants", "the Epistemic closure
behind this node", or "all Class-A requirements under this section". They are
ordinary queries.

### 4.1 Why the query is the right carrier

Three properties of the existing query model make this fit rather than force:

- **The query string is the canonical serialization** of a `QuerySpec`
  (`query_model.md` §9.5.9), already shared across viewer URLs, `{query}`
  directives, and MCP. An anchor is therefore storable as text in a record and
  legible in a diff.
- **The tape is self-contained.** `TapeContent::Edges` carries `output_bids`
  explicitly "so the tape is readable without graph access" (§6.1). Hashing needs
  the member nodes' content hashes and nothing else.
- **Composition is already solved.** Filtered, composed, and mixed-kind scopes
  are ordinary `QuerySpec`s. No scope algebra needs inventing.

### 4.2 Hash the original spec, not the effective spec

`QueryPackage` carries **two** specs (`query_model.md` §6.3):

```
struct QueryPackage {
    original_spec: QuerySpec,   // what the caller asked for
    spec:          QuerySpec,   // what was actually evaluated
    tape:          Tape,
    graph:         Option<BeliefGraph>,
}
```

They differ: `QueryPackage::balanced(spec)` appends halo and section-root
traversal steps so the package graph is self-contained for rendering.

**Anchors must hash `original_spec`.** Hashing the effective spec would make
every anchor sensitive to internal query-planning decisions — adding a halo step,
or optimizing a traversal — so improving the evaluator would stale every
annotation in the corpus. The caller's expressed intent is the scope; the
evaluator's expansion of it is an implementation detail.

### 4.3 The tape hash: order-insensitive by default, with one real exception

Hashing requires reproducibility: the same graph and spec must yield the same
hash.

**Default: canonicalize.** Collect the tape's output BIDs, take each node's
`content_hash`, sort, then hash:

```
tape_hash = sha256(
      canonical(original_spec)
    ‖ sorted( content_hash(n) for n in tape.output_bids() )
)
```

The spec is included so that two scopes containing the same nodes today, but
asking different questions, are distinguishable — a review of "everything under
§3" and a review of "the Class-A items under §3" must not be interchangeable
just because §3 currently contains only Class-A items.

#### Why sorting is a default, not a claim that order is meaningless

The tempting justification — "a *scope* is a set, so order is a rendering
concern" — is too strong, and the distinction matters.

Tape order is not incidental. `query_model.md` §6.1 specifies `TapeContent::Edges`
as "Ordered by `WEIGHT_SORT_KEY` (sibling order)", and §7.3's sort table reads
topological and sibling ordering *directly off the tape* rather than from a
separate pass. `WEIGHT_SORT_KEY` is an **explicit graph property**, not a
traversal artifact: it is persisted on the edge, and reordering peers is already
expressible as a `BeliefEvent` (`PathUpdate` carries an order vector). Combined
with discovery order, it yields a genuine topological property of the graph.

So "the same nodes in a different order" can mean a real change — someone
reordered the document — and an order-insensitive hash will not see it.

#### The two cases, distinguished by where the order comes from

| Order source | Reproducible? | In the hash? |
|---|---|---|
| `WEIGHT_SORT_KEY` + discovery (`TapeContent::Edges`) | **Yes** — a persisted graph property | **Optionally**, per protocol |
| `SortPayload.score` (TF-IDF, decay, boosting) | **No** — see below | **Never** |

**TF-IDF scores must never enter an anchor hash.** IDF is a function of
*corpus-global* term statistics, so a score depends on every other document in
the corpus. Adding an unrelated document anywhere shifts the scores, and
threshold effects can reorder results. An anchor including score order would go
stale on edits to documents it never looked at — the exact false-positive class
§6 exists to prevent. Any tape entry whose order derives from `SortPayload` must
be canonicalized before hashing.

**Structural order is a per-protocol choice.** Whether reordering two sections
should stale a review of their parent is a question about the *claim*, not about
the hash function — "I reviewed these three requirements" survives a reorder;
"I approved this procedure" may not, if step order is the point. The default
remains order-insensitive because it is the weaker, safer assumption, and
because an order-sensitive anchor stales more often. A protocol that needs
sequence-sensitivity should be able to request an order-preserving variant
rather than being silently denied one.

> **Open.** The order-sensitive variant is not specified, and no consumer has
> asked for one yet. What is settled: sorting is a **default with a rationale**,
> not a claim that order is meaningless — and score-derived order is excluded
> unconditionally, because it is not reproducible at all. See §8.

### 4.4 Inherited constraint: bounded traversal

`query_model.md` §11 Q1 notes that `MAX_TRAVERSAL` preserves decidability of
query equivalence: depth-bounded projection corresponds to a decidable
bounded-CRPQ fragment, and removing the cap while retaining `Not`/`Difference`
would not.

Anchors are queries, so they inherit this. "Do these two annotations assert about
the same scope?" is a query-equivalence question, and it is answerable only
because the cap holds. Any proposal to raise or remove `MAX_TRAVERSAL` must
account for anchor equivalence, not only query performance.

---

## 5. The Cached Family

Running a query per annotation to test staleness is a different performance class
from comparing a precomputed field. On a corpus with thousands of annotations,
the general form is not the hot path.

**The common scopes are therefore precomputed and stored per node.** They are
cached instances of §4, not a rival design:

| Field | Scope | Corresponding query (see caveats) |
|---|---|---|
| `content_hash` | the node's own fields | seed only, no traversal |
| `section_hash` | Section-edge closure | `composed_of(*)` ≡ `k-section-s(*)` |
| `epistemic_hash` | Epistemic closure | `k-epistemic-s(*)` |
| `pragmatic_hash` | Pragmatic closure | `k-pragmatic-s(*)` |

**Two caveats on the query column**, both of which mean "corresponding" rather
than "equivalent":

- **Direction.** In the Section model source = child (`query_model.md` §5.2), and
  §5.4 below folds in a node's *sources*. So the closure runs root→leaf —
  `composed_of` / `k-section-s`, not `component_of` / `s-section-k`, which walks
  toward ancestors. The same orientation applies to the other two kinds.
- **Depth.** `MAX_TRAVERSAL` is 10 (`src/query/mod.rs:30`) and `DepthCount::Max`
  clamps to it (`src/query/spec.rs:1274`), so `(*)` is a 10-hop traversal. The
  cached hashes recurse to fixpoint. On a corpus deeper than 10 hops in one kind
  they diverge, and the cache is the more complete answer.

The depth divergence is unresolved and interacts with §4.4, which cites the same
cap approvingly as what keeps anchor equivalence decidable. Either closures adopt
the cap (making cache and query genuinely equal, and §4.4's argument cover both),
or anchor evaluation is documented as exempt (which reopens §4.4). See §8.

An annotation whose scope matches a cached instance stores that field's name and
value; the check is a field comparison. An annotation with an arbitrary scope
stores `(QuerySpec, tape_hash)` and pays for a query at check time.

### 5.1 What is hashed for `content_hash`

`sha256` over the node's content-bearing fields:

| Field | In hash | Rationale |
|---|---|---|
| `bid` | no | identity, not content — the other half of the anchor |
| `kind` | **partly** | see §5.2 |
| `title` | yes | user-visible content |
| `schema` | yes | changes validation semantics |
| `payload` | yes | the structured content |
| `id` | yes | user-authored identity; a rename is meaningful |
| `metadata` | no | §2 |

**The hashes are stored in `metadata`, not `payload`.** `metadata` is defined as
"runtime metadata: per-parse annotations" (`src/properties.rs:1073`) — it is
computed by the compiler, never authored in source, and already survives the full
parse → DB → export → browser round trip. A derived hash is exactly that kind of
data. `payload` is content; a hash *of* the content is not.

This placement is also what keeps the definition non-circular: `metadata` is
excluded from the hash, so a node never hashes a field containing its own hash.
Storing hashes in `payload` would require carving an exception out of the hashed
field set — the placement decision removes the problem instead of managing it.

Use the established underscore convention for compiler-internal keys
(`_query_specs`, `_maps_to_specs`): `metadata["_content_hash"]`, and
`metadata["_section_hash"]` for a closure member.

> **Asset nodes are misplaced, and it is a live defect.** Asset and directory
> nodes carry `payload["content_hash"]` over external bytes
> (`src/codec/builder.rs:4344`, `:4493`, `:4799`). That is the *same notion* as
> the node hash — a content fingerprint — produced by a different codec over a
> different substrate.
>
> Being the same notion, it is **derived data, and derived data belongs in
> `metadata`** (§5.1a). Its current `payload` placement means this section's rule
> hashes a node's own hash: `payload` is in the hash, so on every asset node the
> fingerprint is an input to itself. That is exactly the circularity the
> placement rule exists to prevent, **presently live**.
>
> → **Relocate to `metadata["_content_hash"]`.** Owned by Issue 105 step 3a-pre,
> which must land before anything hashes `payload`.

> **Sibling key: `metadata["_identity_hash"]`.** A second node hash lives in
> `metadata` under the same convention, specified in `content_identity.md`. It
> answers a different question — *is this the same thing?* rather than *did this
> change?* — and the two deliberately have **opposite normalization
> requirements**: identity must survive reformatting so a moved section keeps its
> BID, while staleness must notice change, so `_content_hash` normalizes as little
> as possible. Neither is derived from the other, and neither substitutes for the
> other. See `content_identity.md` §1 and §2.

### 5.1a `metadata` is three tables wearing one name

§5.1 excludes `metadata` from the hash and stores the hash there. Both are
correct, but they are correct for different reasons, and the reason matters
because `metadata` is not one kind of thing. It is currently three, and only one
of them is what §5.1's rationale describes.

| Key | Written at | Derived from | Class |
|---|---|---|---|
| `git` | `builder.rs:989` | repo state | observation |
| `content_profile` | `layout.rs:1071`, `builder.rs:1979` | stemmed `payload.text` | observation |
| `assembly_index` | `layout.rs:216` | graph topology | observation |
| `render_position` | `layout.rs:222-229` | layout solve | observation |
| `structural_weight` | `layout.rs:231` | edge counts | observation |
| `structural_depth` | `layout.rs:235` | network aggregate | observation |
| `_content_hash` | this document | node fields | observation |
| `_identity_hash` | `content_identity.md` | normalized node content | observation |
| `_query_specs` | `md.rs:2500` | **source directives** | **directive cache** |
| `_query_texts` | `md.rs:2507` | **source directives** | **directive cache** |
| `_query_options` | `md.rs:2501` | **source directives** | **directive cache** |
| `_maps_to_specs` | `md.rs:2527` | **source directives** | **directive cache** |
| `sections` round-trip | `md.rs:2634-2648` | frontmatter | **authoring state** |

**Observations** are computed *about* a node from something other than its own
source text — repo state, graph topology, a layout solve. They are Layer 3 data
in the terms of `living_corpus.md` §2: derived, re-derivable, and true only
relative to a particular observation context. §5.1's exclusion rationale is
written for these, and for these it is exactly right.

**Directive caches** are something else entirely. `_query_specs` and
`_maps_to_specs` are the parsed form of `{maps_to}` and query directives the user
wrote *in source*. They are a pure function of L1 content, and they are consumed
at **parse time** by the HTML generator (`compiler.rs:4669`, `:4753`, `:4759`).
They are cached derivations of content, not observations about it.

**Authoring state** is the `sections` frontmatter table — persisted BID and
anchor assignments that round-trip back into the source file. This is L1's own
data passing through the node on its way home.

#### Why the distinction is load-bearing

The classification matters for **placement**, not for hashing — hashing has one
rule for all three (below).

1. **Directive caches must not migrate out of the node.** It is tempting to
   conclude that because most of `metadata` is observational, all of it belongs
   in an annotation layer. It does not. Moving the directive caches to L3 would
   make an L2 parse-time operation depend on L3 — the corpus would not be
   renderable without the annotation server running. That is a layering
   inversion. They are a cached parse of content, and they arguably belong beside
   it in `payload`; that they sit in `metadata` today is likely *why* `metadata`
   reads as a grab bag.

2. **The hash exclusion is right for both, for the same reason.** It is tempting
   to read the directive caches as a gap — two nodes differing only in their
   `{maps_to}` directives would hash identically if `_query_specs` were the only
   record of the difference. It is not the only record. The directive **text** in
   `payload["text"]` is the source of truth, and it is hashed. `_query_specs` is
   an intermediate parse of that text.

   Hashing it would be hashing the same fact twice, in a derived representation
   whose encoding can change without the content changing — a parser refactor
   that alters the cached shape would restate every node's version while the
   source is untouched. That is precisely the false-positive class §6 exists to
   prevent.

#### The rule: hash sources of truth, never derived representations

This is the principle §5.1's field table implements, and it is worth stating
directly because the table alone reads as a list of exceptions.

**A node's version is a function of the authoritative data, not of any cached,
parsed, or computed form of it.** `payload["text"]` is the source of truth for
what a document says; `title`, `schema`, and `id` are sources of truth for how
it is identified. Everything in `metadata` is downstream of one of those, of the
repo, or of the graph — so nothing in `metadata` is hashed. Not because
`metadata` is a special table, but because it happens to contain only
derivatives.

The three-way classification above therefore does **not** produce three hashing
policies. It produces one policy and three reasons it applies:

| Class | Why excluded |
|---|---|
| Observation | derived from repo, topology, or layout — not from this node's content |
| Directive cache | a parse of `payload["text"]`, which is already hashed |
| Authoring state | L1's own data in transit; the source file is authoritative |

**Test for a new `metadata` key**: ask what the authoritative record of this fact
is. If the answer is a hashed field, the key is a derivative — exclude it, and
hashing it would be double-counting. If the answer is *this key itself*, then a
source of truth is living in `metadata` and the placement is wrong — fix the
placement rather than widening the hash.

#### The same test must be applied to `payload` — in the other direction

§5.1 hashes `payload` wholesale. That is only sound if `payload` contains **no
derived or cached data**, and this has not been audited. The classification
above cuts both ways: `metadata` currently holds content (the directive caches),
and `payload` may currently hold derivatives.

What is confirmed present in `payload` today:

| Key | Where | Class |
|---|---|---|
| `text` | `md.rs:2409-2427` | **content** — authoritative |
| `sections` attributes | `md.rs:1223-1226`, `:2634-2648` | **content** — authoritative (see below) |
| `content_hash` (assets) | `builder.rs:4344`, `:4493`, `:4799` | **derived** — should move to `metadata` |
| `listing`, `truncated` (directories) | `builder.rs:4800-4805` | **derived** — a cached read of the filesystem |
| `remote_url`, `branch`, `network_prefix` | `builder.rs:4813-4815` | **derived** — repo state, same class as `metadata["git"]` |

**The `sections` table is authoritative, not a cache** — this is the important
distinction and the easy one to get wrong. A document's `payload` carries
attributes for the sections defined within it, and that table is where a
section heading's **BID is assigned and persisted**. It is L1 authoring state
round-tripping through the node, so it must stay in `payload` and stay hashed.
A reader who sees "frontmatter-derived" and concludes "cache" would delete the
mechanism that makes section BIDs stable.

**On asset `content_hash` specifically** — the asset hash and the node hash are
the same notion produced by different codecs, so the asset hash is **derived
data and belongs in `metadata` for the same reason the node hash does.** Its
current `payload` placement means §5.1 hashes a node's own hash — the
circularity this document's placement rule exists to avoid, presently live on
every asset node. See §5.1's note.

**The rule**: `payload` is for content whose authoritative record is itself.
Anything cached, computed, or observed moves to `metadata`. Once that holds,
hashing `payload` wholesale is safe by construction rather than by inspection.

> **Owner: Issue 105, step 3a-pre.** Auditing `payload` and relocating its
> derived keys is a prerequisite for trusting §5.1's `payload: yes` row, and it
> pairs with the `metadata` classification above — one reclassification, not two.
> It sits in Issue 105 because that issue computes the hashes that break without
> it. `parse_sections_metadata` (`md.rs:1223-1226`) is the function to read
> first: it decides what enters a node from frontmatter, and it is the boundary
> where authoring state and cached derivation are hardest to tell apart.

**Worked example — source ranges.** Issue 103 proposed `metadata["_source_range"]`
and then rejected it. A byte range is derived from the file's layout, and the
file is authoritative — so it excludes cleanly from the hash under the rule
above. But excluding it from the *hash* is not the same as excluding it from
*equality*:
`BeliefNode::PartialEq` includes `metadata` (`src/properties.rs:1099`) and
incremental parse compares whole nodes (`src/beliefbase/base.rs:849`), so a
whitespace edit shifting every range would fire a `NodeUpdate` per node. The key
went into an anchored side table instead. **The lesson generalizes: hash
exclusion and equality exclusion are different questions, and `metadata`
currently answers only the first.**

### 5.2 Load-state kinds must be excluded

`BeliefKindSet` can carry `BeliefKind::Trace`, which marks a node whose relations
are only partially loaded (`src/properties.rs:520-532`) and which is **stripped
on merge**. Hashing it means a node hashed while Trace and rehashed once complete
produces two different versions — silently staling every annotation on it.

Hash only content-bearing kinds. Audit `External` on the same grounds. This is a
correctness requirement, not an optimization: without it, shard hydration
(Issue 66) invalidates the corpus.

### 5.3 Stratification: why closures are per edge kind

A closure hash walks a subgraph. It must terminate, and it must terminate on real
corpora rather than on idealized ones.

**Per-kind, not across kinds.** `BeliefBase` invariant 0 aims for acyclicity
*within* each `WeightKind` — three independent SCC checks, one each for Section,
Epistemic, and Pragmatic (`src/beliefbase/base.rs:237`, `1198-1223`). The union
carries no such property: section A contains B, B `{maps_to}` A is legal and
unremarkable. A closure over mixed kinds would not terminate. Confining each hash
to one kind's subgraph via `BeliefGraph::as_subgraph(kind, reverse)`
(`src/beliefbase/graph.rs:164`) makes the mixed-kind cycle unreachable.

**A visited set is still required.** Invariant 0 is *reported*, not enforced:
`built_in_test` collects violations into a diagnostics vector and sets
`balanced = false` (`base.rs:1350-1360`); nothing rejects the graph, and the
check is not on the parse path. Same-kind Section cycles do occur —
`PathMap::new_indexed` maintains a `loops` set from `DfsEvent::BackEdge`
specifically "to prevent infinite recursion during path generation"
(`src/paths/pathmap.rs:1852`, `2035`, `2147`). A hash without a visited set would
hang on precisely the corpora that code exists to survive.

Stratification bounds the mixed-kind blowup; the visited set handles same-kind
cycles, parallel edges, and self-loops. Both are needed.

Take `kind` as a parameter and never union edge kinds inside the traversal. A
debug assertion that the walk stays within `kind` makes a future violation fail
loudly rather than hang.

### 5.4 Computation

```
content_hash(n) = H( content-bearing fields of n )

kind_hash(n, k) = H( content_hash(n)
                   ‖ kind_hash(s, k) for s in sources(n, k), canonical order )
```

Bottom-up with memoization: **each node is hashed exactly once**, one post-order
traversal, O(V+E) per kind. Children contribute their computed hashes, not their
contents — this is what git does for tree objects. Nodes may have several parents
within a kind, so this is a DAG; memoization handles shared substructure.

**Cost**: 32 bytes of raw digest per node per kind — ~4 MB for four fields on a
32k-node corpus. **That is the in-memory floor, not the delivered cost.** These
are stored hex-encoded in a `toml::Table` (§5.1) and round-trip through the DB
and shards (§9), so budget roughly 80 bytes per entry with its key — on the order
of 10 MB, not 4. Measure before treating either number as a constraint.

**Subnet boundaries are ordinary Section edges.** A closure does not stop at a
network boundary, so a network index node's `section_hash` covers its whole
network. This is a semantic decision, not an implementation detail: it means
editing any document in a subnet stales a section-scoped claim on the parent
network.

**Base case is implicit.** A node with no sources on a kind folds in nothing, so
its `kind_hash` equals its `content_hash`. No sentinel, and the degenerate case
is also the correct one: for a node containing nothing, "did anything under me
change?" and "did I change?" are the same question.

### 5.5 The leaf collapse, and what it breaks

The base case is a trap for the caller, not for the implementation. Because a
leaf's `kind_hash` equals its `content_hash`, **anchoring a claim to the wrong
cached field can be silently wrong rather than merely imprecise.**

The concrete case: a verification attestation on a requirement. Requirements are
typically Section leaves. Anchoring it to `section_hash` means anchoring to
`content_hash`, so the attestation detects edits to the requirement's own text
and *nothing else* — including changes to the evidence it rests on. That is the
one thing a verification claim exists to notice.

The correct anchor is the Epistemic closure. This is why the family cannot be
trimmed to `content_hash` and `section_hash`, the two structurally obvious
members: the third is load-bearing for the compliance case.

### 5.6 Cross-kind closures are not a composition

`H(section_hash ‖ epistemic_hash ‖ pragmatic_hash)` is well-defined and cheap,
and it does **not** answer "did anything upstream of me change, along any edge".

Counterexample: A contains B (Section), B draws on C (Epistemic). C is in neither
A's Section closure nor A's Epistemic closure, so no combination of the two
detects a change to C. Mixed-kind reachability is exactly what the cross-kind
question asks about, so the composition answers a strictly weaker question while
appearing to answer the full one.

A mixed-kind scope is expressible — as a `QuerySpec` that traverses mixed kinds
(§4). It is not expressible as a combination of per-kind caches. Use the general
form.

---

## 6. Consequences

**Editing node A does not stale annotations on node B**, unless B's scope
contains A. This is the property the whole-corpus hash could not provide, and the
reason the signal carries information.

**Revert restores currency.** A node edited and then reverted returns to its prior
hash, and annotations anchored there become current again. Correct — the content
is genuinely identical.

**`asset_version` survives as informational.** Records may carry which build the
actor was viewing. It is never the anchor.

**Determinism is a hard requirement.** The hash must be stable across processes,
platforms, and a rebuild from shards. Fixed field order, canonical serialization,
no map-iteration-order dependence. An unstable hash stales every annotation on
every restart, which is indistinguishable from the whole-corpus failure mode
§2 rejects.

Two known hazards for determinism:

- `Table` is `toml::map::Map`, BTreeMap-backed under current lockfile features,
  so it iterates in sorted key order. Enabling `preserve_order` anywhere in the
  dependency graph would switch it to `IndexMap` (insertion order) and silently
  destabilize every hash. Pin or assert this.
- `payload["text"]` is regenerated from the markdown event stream and only
  rewritten when `inject_context` fires (`src/codec/md.rs:2380-2395`). The hash
  inherits any nondeterminism in that path, so the stability test must cover the
  full parse → DB → export → hydrate round trip, not repeated hashing of one
  in-memory node.

---

## 7. Consumers

| Consumer | Uses | Why |
|---|---|---|
| Annotation anchoring (Issues 104, 105) | the selected scope's hash | staleness of claims |
| Attestation server (Issue 65) | per-node hashes in the DOM | anchoring on a static site |
| Incremental parse (Issue 66) | per-file source hashes | skip clean networks |
| Cross-version diff (Issue 74) | `content_hash` as a join key | change detection |
| Evidence citation (Issue 108) | span hashes over external stores | verify a citation still holds |
| Node source ranges (Issue 103) | per-file source hashes | validity key for an anchored range table |
| Codec-regression detection | per-node hashes, compared across binaries | detect behavioural drift in out-of-tree codecs (§7.2) |

Incremental parse and cross-version diff consume content versioning with no
annotation involvement. This is a noet-core capability, not an annotation
feature.

### 7.1 Three scopes, three consumers — do not conflate them

The consumers want different things, and an early draft of Issue 66 merged two of
them:

| Consumer | Question | Scope | Owner |
|---|---|---|---|
| Incremental parse | "must I re-parse this file?" | per-**file**, no traversal | Issue 66 (`source_hashes`) |
| Cross-version diff | "is this the same node?" | per-node, no traversal | Issue 105 (`_content_hash`) |
| Annotation anchoring | "did what I reviewed change?" | per-node, scope-dependent | Issue 105 (closures) |

The split is by *consumer*, and only the per-**file** hash is Issue 66's. The
entire per-node family — radius 0 and closures — belongs to Issue 105, the first
issue in the build sequence that needs one; that keeps the issue gating
everything else small.

**Closure hashes are annotation infrastructure, not parse infrastructure.** A
closure recurses to fixpoint and traverses subnet boundaries (§5.4), so editing
one leaf changes every ancestor's hash to the network root. For anchoring that is
correct — "I reviewed this section" *should* stale when a child changes. For skip
logic it is the opposite of what is wanted: it would invalidate the whole
ancestor chain and defeat the skip the incremental parse exists to enable.

So the split is by consumer, not by convenience of implementation:

- **Issue 66** computes per-file `source_hashes` (skip logic) and per-node
  `_content_hash` (radius 0 — no traversal, needed by cross-version diff and by
  every annotation kind as its base case).
- **Issue 105** computes the closure members it needs, when it needs them, having
  the `protocol_id` → scope mapping that determines which.

Export walking every node makes it *tempting* to compute closures there too. That
is an argument about where the loop is, not about who needs the value, and it put
graph traversal on the critical path of the issue that gates everything else.

### 7.2 Codec-regression detection, and the constraint it imposes

A node hash answers "did this content change?" Holding the content fixed and
varying the *compiler* instead turns the same value into a regression detector:
if the same bytes produce a different node hash under a new binary, a codec's
behaviour changed.

This matters because the codec registry is a public extension surface —
`CODECS.insert_codec`, `WALK_CODECS.register`, and `CLAIM_MAP.claim`
(`src/codec/mod.rs:1145`, `:403`, `:548`) — consumed by out-of-tree binaries. The
`DocCodec` trait is small, so *signature* breaks are caught by the compiler. The
undetected class is **behavioural**: the trait still compiles, the codec still
runs, and the nodes it emits are subtly different. No type system sees that; a
stored hash manifest does.

The mechanism is a corpus fixture plus a manifest of expected node hashes — no
new machinery, just a golden test over a value this document already defines.
Deliberately **empirical rather than declared**: a `DocCodec::version()` would
detect only the drift an author remembered to declare, whereas a hash manifest
detects actual output drift. Do not add a codec versioning scheme for this.

**The constraint this imposes on the hash**: it must be comparable **across
binaries**, not merely across runs of one binary. Nothing binary-specific may
enter the hash input — no pointer values, no allocation-order-dependent
iteration, no build timestamps, no compiler version. This is a strengthening of
§6's determinism requirement, which is otherwise satisfiable by a hash that is
stable within a build but not between builds.

This consumer is listed so the hash is not designed in a way that precludes it.
It has no issue and should not get one until per-node hashes land.

---

## 8. Open Questions

- ~~**Which family members are computed in Phase 1.**~~ **Partly resolved**:
  §7.1 assigns `_content_hash` to Issue 66 and *all* closure members to Issue 105,
  because closures serve annotation anchoring rather than parse skip logic.

  What remains open is which closures **Issue 105** computes. §5.5 argues the
  Epistemic member is required for attestation correctness — a requirement leaf's
  `_section_hash` equals its `_content_hash`, so a section-anchored attestation
  cannot see its evidence move. Counter-argument: an Epistemic closure over a
  densely-linked corpus may fire constantly, which is §2's always-fires failure in
  a new form. Computing it is the only way to get fire-rate data. Recommend
  computing it in Issue 105 while treating closure-scoped attestation anchoring as
  provisional.
- ~~**`content_hash` collides with an existing payload key.**~~ **Resolved**:
  node hashes live in `metadata` under underscore-prefixed keys
  (`_content_hash`, `_section_hash`) — see §5.1. `metadata` is derived per-parse
  data and is hash-excluded, so the placement is both semantically right and
  non-circular. The asset-node `payload["content_hash"]` is the **same** notion
  and must relocate to `metadata` for the same reason — not optional, and not
  future work: while it sits in `payload` the hash is circular on every asset
  node. Owned by Issue 105 step 3a-pre.
- **Node deletion.** Deleting a Section source changes every ancestor's
  `section_hash`, so closure-anchored annotations stale correctly. An annotation
  anchored *directly* to the deleted node orphans, and nothing specifies whether
  that surfaces as stale, broken, or silent. Structurally identical to the BID
  migration question in `content_identity.md` §8; answer together.
- **`{maps_to}` resolution is not content-expressed.** The premise that content
  hashes capture relations for free holds for the directive text, but the resolved
  spec lives in `metadata["_maps_to_specs"]` (hash-excluded by design) and the
  edge is third-party-owned via `WEIGHT_OWNED_BY` — so it appears in neither
  endpoint's `content_hash`. Matters most for the Epistemic case, since
  `{maps_to}` is the traceability primitive.
- **Document nodes are not purely content-scoped.** `MdCodec::finalize` writes a
  `sections` table into the document node's frontmatter carrying each child's
  `bid`/`id`/`schema` (`src/codec/md.rs:2630-2695`), and frontmatter becomes
  `payload`. A document node's `content_hash` therefore already moves when a child
  is added, removed, or renamed — though not when a child's prose is edited. The
  argument for a separate closure hash survives (in-place edits are the common
  case), but the "own fields are entirely unchanged" framing is too absolute for
  document nodes.
- **Radius between 0 and closure.** The family caches radius 0 and radius ∞. A
  depth-1 or depth-2 scope is expressible as a `QuerySpec` but not cached. Whether
  any is common enough to warrant caching needs usage data.

- **Do cached closures respect `MAX_TRAVERSAL`?** §5.1's query column and §5.4's
  fixpoint recursion disagree beyond 10 hops. §4.4 leans on the cap for anchor
  equivalence decidability, so exempting closures weakens that argument while
  capping them makes a "closure" not a closure. **Decide before implementation** —
  it changes what Issue 66 computes.

- **Hash input serialization is unspecified.** §5.1 names the fields and §6
  requires canonical encoding, but no encoding is named. `payload` and `metadata`
  are `toml::value::Table`, `id` is a custom type, `kind` an `EnumSet`. The
  encoding must be length-delimited, or `title="ab", id="c"` and `title="a",
  id="bc"` collide. Needs a named encoding, not a principle.

- **Which `kind` values are content-bearing.** §5.2 excludes `Trace` and says to
  audit `External` — but `External` is not obviously load-state; it marks a node
  wrapping an unparseable reference, which is arguably content. The builder cannot
  defer this. Needs the explicit set.

- **Source ordering for Epistemic and Pragmatic closures.** §5.4 folds sources in
  "canonical order". For Section that is `pathmap_order`
  (`src/paths/pathmap.rs:476`). For the other two kinds nothing is specified;
  `WEIGHT_SORT_KEY` on the edge is the likely answer but must be stated, since an
  unstable fold order destabilizes the hash.

- **Which tape lens `tape_hash` uses.** §4.3 hashes "the tape's output BIDs", but
  `query_model.md` §6.3 requires choosing a `TapeFn` lens — `Then(None)` yields
  the final frontier, `Fold{Union, None}` everything discovered at any depth. For
  a closure query those differ enormously. Either name the lens or state that it
  is part of the `QuerySpec` and the anchor inherits it.

- ~~**Where the hashes are stored.**~~ **Resolved**: `metadata`, under
  underscore-prefixed keys (§5.1). No new `BeliefNode` field, so no breaking
  serde change; `metadata` already round-trips through the DB and shards.

- **Should the asset and node content hashes eventually unify?** They are the
  same notion — a content fingerprint — produced by different codecs over
  different substrates. A unified model would have codecs supply the hash for
  content noet does not parse and the compiler derive it for content it does.
  Not required; revisit if a third substrate wants one.

- **Incremental invalidation.** §5.4 gives the from-scratch cost. Editing one node
  should invalidate only its ancestors along that kind — O(depth), not O(n) — but
  the rule is unstated, and its interaction with the visited set is non-obvious
  (a cycle-broken edge means the ancestor set is not a clean tree walk). A watch
  loop needs this.

---

## 9. Implementation Map

| Element | Status |
|---|---|
| Per-file `source_hashes` (skip logic) | **Issue 66** step 1 |
| Unconditional `payload["text"]` derivation | **Issue 66** step 1c — precondition for hashing `payload` at all |
| `_content_hash` per node (radius 0) | **Issue 105** step 3a |
| Kind-parameterized hash function | **Issue 105** step 3a |
| `_content_hash` in shard records | **Issue 105** step 3a |
| Closure members (`_section_hash`, Epistemic, Pragmatic) | **Issue 105** step 3b — see §7.1 |
| Closure hashes in the DOM | **Issue 65**, after Issue 105 |
| `protocol_id` → scope selection | **Issue 104**, **Issue 105** |
| `(QuerySpec, tape_hash)` general form | unowned — needs an issue |
| Tape canonicalization for hashing | unowned — needs an issue |
| `payload`/`metadata` reclassification (§5.1a) | **Issue 105** step 3a-pre — blocks 3a |
| Identity-hash normalization | specified in `content_identity.md`; built by **Issue 36** — `_content_hash` deliberately does not normalize |
| Codec-regression hash manifest (§7.2) | unowned by design — not before per-node hashes land |


The general form has no owner. It is not required for Phase 1 — the cached family
covers the annotation kinds Issues 104 and 105 ship — but the filtered-scope case
in §3 has no other answer, and the model should not be re-derived when it is
wanted.

---

## 10. References

- [`query_model.md`](../core/query_model.md) — §3 `QuerySpec`, §6 the Tape, §6.3
  `QueryPackage` and the two-spec distinction, §9.5.9 canonical serialization,
  §11 Q1 decidability
- [`living_corpus.md`](../annotation/living_corpus.md) — §4 anchoring and scope selection in
  the annotation model
- [`dag_model.md`](../core/dag_model.md) — the three edge kinds
- [`attestation_fabric.md`](../annotation/attestation_fabric.md) — §6 protocol registry, the
  home for scope selection
- [`collaboration_overlay.md`](../annotation/collaboration_overlay.md) — §3.2, the superseded
  `asset_version` argument
- [`../essays/engineering_model_ontology.md`](../../essays/engineering_model_ontology.md)
  — §6.1, why `WeightKind` has three variants and `R` is not a fourth
- [`beliefbase_architecture.md`](../core/beliefbase_architecture.md) §4.3 — the event
  and record schema, including the `Envelope` an anchor travels in
- `src/properties.rs` — `BeliefNode`, `PartialEq`, `BeliefKind::Trace`,
  `WeightKind`
- `src/beliefbase/base.rs` — invariant 0 and the SCC checks
- `src/beliefbase/graph.rs` — `as_subgraph`
- `src/paths/pathmap.rs` — `pathmap_order` (L476), the `loops` cycle guard
