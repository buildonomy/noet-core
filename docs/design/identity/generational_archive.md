---
title = "Generational Archive: Retained Graph State and Structural Diff"
authors = "Andrew Lyjak, Claude"
last_updated = "2026-09-13"
status = "Draft"
version = "0.1"
dependencies = ["content_identity.md", "content_versioning.md", "codec_determinism_contract.md"]
---

# Generational Archive

## 1. Purpose

A corpus that can only describe its current state cannot answer the question
every reader eventually asks: **what changed?**

This document specifies the retained state that makes that answerable, and the
procedure that turns two graph states into an intelligible account of the
difference between them. It is one mechanism with three user-facing consumers:

| Consumer | The two sides being compared |
|---|---|
| **Version history** | an archived generation vs. the current corpus |
| **Staleness detail** — "what changed since I read this?" | the generation a receipt anchors to vs. current |
| **Redlines** — a proposed change, before it lands | the corpus vs. the corpus with a redline applied |

These look like three features. They are one comparison with three sources for
its *old* side, and the quality bar is identical in all three.

> **The bar is intelligibility, not just correctness.** A comparison that reports a
> moved section as a deletion plus an addition is correct and useless. It reports
> churn instead of change, and a reader who must re-read the document to find out
> what happened has paid the cost the receipt existed to remove.

**Scope.** This document specifies the archive format, the identity basis for
comparison, the move-detection procedure, and the layer each part belongs to. It
does not specify retention policy (which generations survive), intra-node text
diffing, or the render.

## 2. What is already fixed

Three constraints come from elsewhere and are not open here. Recording them
shrinks the design space considerably.

**The event vocabulary is sufficient.** A move is expressible today: relation
changes removing the old placement, adding the new, and re-indexing displaced
peers. `PathUpdate(bref, path, bid, order, origin)` carries an order vector, and
`RelationChange` / `RelationRemoved` carry the rest (`src/event.rs`). A
`BidMigration` shorthand is worth having for *legibility of intent*, but no new
variant is required to express a move.

**`compute_diff` has the right signature.** It takes two `&BeliefBase` plus a
scope and returns `Vec<BeliefEvent>` (`src/beliefbase/base.rs`). The
archive's job is to supply an old side that *is* a `BeliefBase` — not to invent a
second comparison entry point.

**Two hashes exist, and neither derives from the other.** This is the fact the
whole format turns on.

| | `_content_hash` | `_identity_hash` |
|---|---|---|
| Question | "did this change?" | "is this the same thing?" |
| `id` field | included | **excluded** — load-bearing |
| Links | raw | **collapsed** to a `NodeKey` |
| Normalization | minimal by design | aggressive |
| Specified in | `content_versioning.md` §5.1 | `content_identity.md` |

`content_identity.md` §2.3: both keys coexist; neither substitutes for the other.

## 3. The archive format

### 3.1 Stub shards plus a blob cache

An archived generation is a set of **stub shards** — shards with node bodies
replaced by hashes — alongside a content-addressed blob store holding the bodies:

```
ArchiveShard {
    network_bref,
    stubs:     BTreeMap<Bid, (ContentHash, IdentityHash)>,
    relations: SerializableBidGraph,
}
```

**Why relations are stored whole.** A bare blob store cannot work. `compute_diff`
takes a `&BeliefBase` — a graph with relations, indices, and paths — and a pile
of node bodies is none of that. Reconstructing edges would mean re-deriving them
from `payload.text` with *machinery that is not the parser*: a second parse
implementation whose output must agree with the first forever. That violates
`codec_determinism_contract.md` G1 and G2 outright. Storing relations sidesteps
it — stubs resolve to blobs, the pair hydrates through the **existing** shard
path, and `compute_diff` receives the type its signature asks for.

**A blob is a parsed node, not source text — and that is why node granularity
works.** §6.1 requires a *redline's* payload to be whole files, because proposed
content must be **parsed** and a fragment parse yields a node whose identity hash
differs from the same text in situ. None of that applies here. An archive blob
is a `BeliefNode` that was already parsed, in full document context, at archive
time; recovering it is a deserialize, not a parse. The shard format already
stores nodes this way — `NetworkShard.states` is a `BTreeMap<String, BeliefNode>`
(`src/shard/wire.rs`) — and hydration is `From<BeliefGraph> for BeliefBase`, an
index build with no codec involved.

The two rules therefore point in opposite directions without conflicting:
**store source at file granularity, store parsed state at node granularity.**
The dividing line is whether a parser has to run.

This is also the sharpest divergence from git. Git's dedup unit is the file, so
a one-line edit to a 7,600-line document re-stores the whole file. noet's unit is
the node, because noet has already paid the parse cost and knows where the
boundaries are — the same edit re-stores one node body and shares the other
several thousand. For this corpus that is a finer saving than delta compression
would buy, and it needs no chain.

**Two addressing schemes, because two objects have different keys.** A node's
`content_hash` covers its own fields and *not* its edges, which is why closure
hashes exist (`content_versioning.md` §5.4) and why a pure re-parent does not
move a node's hash. So `content_hash → (node, edges)` is a key that does not
determine its value:

| Object | Keyed by | Dedup |
|---|---|---|
| **blob** (node body) | `content_hash` | global; an unchanged node is one entry across all generations |
| **stub shard** (relations + hashes) | generation × network | by shard-file hash; an unchanged network is one entry |

### 3.2 Why the stub carries `identity_hash`

A stub keyed on `content_hash` alone **cannot serve §1's requirement**.

A section that moves keeps its `_identity_hash` but routinely *moves* its
`_content_hash` — because `id` is included in one and excluded from the other,
and because link text is raw in one and collapsed in the other. Both change when
a section relocates. So a `content_hash`-only archive reports every move as
remove-plus-add: exactly the churn the archive exists to prevent.

With both hashes present, **an exact move is a stub-level set operation** — same
`identity_hash`, different parent — answerable from the stub shard alone, since
relations are stored whole. No blob fetch. Blobs are then touched only for the
question that genuinely needs bytes: *what text* changed.

**The stub carries no kind set**, though a read path filtering on Trace might
seem to want one. It does not need it: **at rest, `Trace` implies `External`**,
and `External` is recoverable from the blob on the rare occasion a filter needs
it.

The two are different things wearing one flag. `External | Trace` marks a
*permanently* incomplete node — an href, an asset, the API node, a namespace root
— where "no deeper cache fetch will yield a non-Trace version"
(`src/properties.rs`). Bare `Trace` marks partially-loaded relations: an
in-transit condition, stripped on merge, which should never reach storage.
`export.rs` already states the intent — "Trace nodes introduced by balanced
traversal (cross-network references) are excluded."

Dropping the field is the cheap direction even if a bare-Trace node does slip
through. The cost is a **frivolous blob fetch**, which is recoverable and
cacheable; the cost of carrying the field is paid on every stub of every
generation forever. What matters is that the failure mode is **loud** rather than
silent — see the warning §3.4 requires.

### 3.3 Cost

On a measured corpus of 2,445 shards (95.5 MB of network shards), node states are
**74–82%** of a shard and relations **18–26%** — sampled at the largest shard
(9.3 MB, 5,664 nodes) and at the median (15.5 kB, 17 nodes). A stub shard is
therefore roughly **a quarter** the size of the shard it archives, before
cross-generation dedup. A one-node edit in that largest network costs ~2.4 MB of
retained relations plus one new blob, against 9.3 MB for a full retained shard.

The two extra digests cost ~64 bytes per node against that baseline —
negligible, and they are what make §4 stage 2 possible.

### 3.4 Two checks before the format is frozen

- `SerializableBidGraph` is BID-keyed (`src/shard/wire.rs`) with no
  dependency on the node table, so it should round-trip standalone. Assert it
  with a test rather than assuming.
- **Warn loudly on a bare-`Trace` node at rest.** Keeping no kind set in the stub
  (§3.2) accepts a frivolous blob fetch if one appears; what it must not accept
  is that appearing silently. Export and hydration should each log a bare-`Trace`
  node as an integrity violation naming the node, since it means some machinery
  failed to complete a node it should have. Issue 66 owns the invariant and both
  crossings.

  **The invariant most likely already holds**, which is what makes §3.2's omission
  safe rather than optimistic. The SPA has been loading shards on demand across a
  ~2,445-shard corpus, driven by `bref_index` lookups and `get_context` misses; a
  node reporting `is_complete() == false` when it is in fact complete produces
  spurious fetches and unresolvable metadata panels — a failure class that
  surfaces immediately in a browser, as the 17 MB global-shard incident showed.

  **The DB path is the untested half.** That evidence covers shards. A
  `DbConnection` carrying bare-Trace rows would degrade a query result rather
  than freeze a page, which is quieter and therefore likelier to have gone
  unnoticed. Assert on both crossings, not just the shard one.

## 4. Move detection: a three-stage ladder

Each stage runs only on what the prior stage could not resolve. This is what
keeps the expensive case off the common path.

| Stage | Mechanism | Cost | Resolves |
|---|---|---|---|
| **1. Naive diff** | `compute_diff` as today — everything is add/remove | present cost | all pure adds and removes |
| **2. Exact move** | match the remainder by `identity_hash` | stub-level; no blobs | clean move, refactor, copy |
| **3. Fuzzy move** | TF-IDF similarity over the stage-2 remainder | bounded blob fetch | move-with-edits → move **then** edit |

**Stage 3 reuses shipped machinery.** `src/shard/search.rs` already implements
tokenization with stop words and Snowball stemming (`tokenize`, `:600`;
`Stemmer`, `:537`) and smoothed Laplace IDF scoring (`:888`). A move-with-edits
candidate is a nearest-neighbour query over a small document set — the same
computation the search index performs, against a different corpus. **No new
similarity engine.**

In an ordinary edit session the stage-2 remainder is nearly empty, so stage 3
costs nothing. A corpus-wide reorganization pays for it; a typo does not.

**Threshold and confidence.** Stage 2 is exact: confidence 1.0, automatic. Stage
3 produces a similarity score, which `BidMigration.confidence: f32` carries. The
threshold question is therefore scoped to stage 3 alone.

**A precondition the ladder depends on.** Stage 2 is sound only if *both* sides
carry computed hashes. An archived generation does by construction. Proposed
redline content does not — see §6.

## 5. Where each part lives

**Detection stages 2 and 3 belong to `DocumentCompiler`, not `GraphBuilder`.**
This is structural, not stylistic.

`GraphBuilder::parse_content` operates on one file (`src/codec/builder.rs`).
A move is by definition a cross-file observation — the section left document A
and arrived in document B — and in a parallel epoch those files are parsed by
different tasks that never see each other's results. A builder-level matcher
would be asked "is this a move?" at a point where the other half of the answer
does not exist.

`DocumentCompiler` is the only component that spans epochs, and it already holds
structurally identical cross-file indices:

| State | Purpose | Site |
|---|---|---|
| `proto_index` | pre-parse index of every document | `compiler.rs:173` |
| `absorbed_to_claimant` | cross-file BID absorption across a run | `:223` |
| `parsed_node_paths` | `Bid` → the path that produced it | `:237` |
| `remainder_queue`, `processed` | multi-epoch reparse bookkeeping | `:182`, `:183` |

**The precedent is already at this layer.** Asset dedup — the same
hash-collision mechanism over a different substrate — lives in
`DocumentCompiler::create_asset_hardlinks` (`compiler.rs:5467`), not in the
builder, because it compares a content hash against every *other* file's
canonical entry.

So the work decomposes:

| Responsibility | Layer | Why |
|---|---|---|
| Compute and attach `_identity_hash` | `builder.rs` | a pure function of one node's content |
| Stage 1 — naive diff | `compute_diff` | already takes two whole `BeliefBase`s |
| Stages 2, 3 — move detection | `compiler.rs` | needs run-wide add/remove sets |
| Write the archive generation | `compiler.rs` | happens after a run completes |

The builder computes hashes; the compiler interprets them across files.
Detection and archival therefore sit at the same layer and share one index.

## 6. The third source: a redline's proposed content

Version history and staleness both compare two states that have **been parsed**.
A redline has not, and that difference propagates further than it first appears.

A redline's payload is a proposed change to content an author wrote by hand. It
has no BID, no `_content_hash`, no `_identity_hash`, and no edges — nothing has
computed them, because nothing parsed it.

This breaks §4 stage 2 directly. Worse, it breaks it *subtly*: the link-collapse
rule (`content_identity.md` §5) resolves each link to a `NodeKey::Bref` **when
the compiler located the target**. On unparsed text there is no resolution, so
the fallback key applies — and proposed text identical to an existing section can
hash differently purely because its links were never resolved.

### 6.1 A redline is a map of file path to content

**The unit is the file.** A redline is
`BTreeMap<corpus-relative path, content string>`, parsed through the ordinary
`GraphBuilder` to produce a candidate graph state.

Two independent arguments force this, and they converge:

**Determinism.** `codec_determinism_contract.md` G1/G2 require node output to be
a pure function of content. Computing hashes for proposed text by any route other
than the real codec is a second parse implementation — the same violation §3.1
rejects for the blob store. The proposed content must go through the same
machinery, or its hashes are not comparable with the corpus's.

**Anchoring.** A section's identity depends on its document context: its parent
stack, its sort key, its resolved links. Parsing a fragment yields a node whose
`_identity_hash` differs from the same text in situ, purely from absent context.
**The file is the smallest unit at which the parse is total**, and therefore the
smallest unit at which the resulting diff is well-anchored. A redline that
claimed section granularity would be claiming an anchor it cannot compute.

This mirrors git, where the blob is the file and hunks are a presentation layer.
An authoring surface may present a section-level edit; the record stores the
file.

### 6.2 The candidate graph state must never be stored

Diff pairs nodes by BID, so a candidate carrying proposed content reuses the
base's BIDs. That is safe **only** because it is a rendering artifact: it lives
in one `QueryPackage`, is discarded after the read, and is never merged back.
Persisting it would put two accounts of one node's content in a store with
nothing to reconcile them, and `BeliefGraph::union_mut`
(`src/beliefbase/graph.rs`) would resolve the collision by destroying the
base node.

`overlay_model.md` §2.5 states the rule and §2.6 the constraint behind it.
Nothing in the type system distinguishes a safe candidate from an unsafe one, so
it must be asserted by test: **a candidate package is unreachable from any
`BeliefSource` once the render completes.**

### 6.3 Consequence: the candidate shape is required, not preferred

Issue 74 records a fork — compare the redline's proposed text against the
target's current text (two-payload), or build a candidate graph state. The
two-payload form was already the weaker option on coverage grounds. §6.1 makes it
*insufficient*: it never produces a comparable node, so it structurally cannot
answer "did this section move." The candidate state is the only form that gives
proposed content the hashes a move-aware diff needs.

### 6.4 The archive and a redline together give a three-way diff

A redline carries the `corpus_version` it was written against (§6.1). When the
corpus has moved past that generation, the archive can supply it — and the
result is a merge base:

```
           V1   the generation the author saw (from the archive)
          /  \
        V2     R
   corpus now   proposed
```

**Without V1 there are only two sides, and two sides cannot distinguish a
proposal from drift.** Given V2 and R alone, text that differs might be what the
author changed or might be what the corpus changed underneath them. Those need
opposite handling: the first is the proposal, the second is staleness the author
never intended to assert. With V1 the classification is the standard one:

| V1 → V2 | V1 → R | Reading |
|---|---|---|
| unchanged | changed | the proposal — apply it |
| changed | unchanged | corpus drift; the redline is silent here, keep V2 |
| unchanged | unchanged | untouched |
| **changed** | **changed** | **genuine conflict — surface it** |

This sharpens staleness from a flag into an answer. `redline_model.md` §4 makes
staleness a look-time comparison that tells a reader the target moved; three-way
tells them **whether the movement actually conflicts** — the difference between
"re-read this before submitting" and "these are compatible, proceed." A redline
sitting in a change package for weeks, which §4 of that document calls the longest
natural latency in the system, is exactly the case where most drift will be
compatible and flagging all of it as stale is noise.

**This is a capability neither piece has alone**, and it is a second consumer of
the archive beyond "what changed since I read this?" It also gives §9.3's
retention tiers a sharper consequence: a trail whose base generation is
*recomputable* still supports conflict detection after a re-derive, and one whose
base is *lost* degrades to a two-way diff — still renderable, no longer able to
tell a proposal from drift. That is a functional loss, not only a UX one.

Owned by **Issue 74** with the rest of the comparison machinery; the archive's
only obligation is to resolve a `corpus_version` to a graph state.

## 7. Client-side parse and the WASM boundary

The candidate graph state is cheap to build server-side. Building it in a browser
— which is what a static-site redline surface would need — is **close but not
currently possible**, and the gap is narrow enough to be worth stating precisely.

Compiling the crate for `wasm32-unknown-unknown` with the codec gates lifted
shows that `builder.rs` and `md.rs` produce **no errors of their own**. All 24
`tokio::fs` failures land in `compiler.rs`. The existing blanket
`#[cfg(not(target_arch = "wasm32"))]` on `pub mod builder` (`src/codec/mod.rs`) is
inherited caution rather than a measured incompatibility, and it corroborates §5:
the compiler is the filesystem layer, the builder is the pure parse layer.

The one genuine barrier:

```
error[E0277]: `Rc<RefCell<BidGraph>>` cannot be sent between threads safely
    --> src/codec/builder.rs:3611
     |  BeliefSource::evaluate(&self.session_bb, &mut package).await?;
```

`BeliefBase` uses `Rc<RefCell<…>>` on wasm32 and `Arc<RwLock<…>>` natively (the
`SharedLock` alias, `src/beliefbase/base.rs`), so on WASM it is not `Send`
— while `BeliefSource::evaluate` is an `async fn` carrying a `Send` bound. The
builder is single-threaded by nature, so this is a bound too strong for the
target rather than a design conflict. Candidate resolutions: a `?Send` async
variant, `spawn_local`, or making the bound conditional on target.

Secondary, mechanical: `MdCodec::proto` reads frontmatter via `File::open`
(`src/codec/md.rs`) and needs a content-taking entry point; the `read_dir`
in `process_asset_dir` is never reached by a redline.

**Why this matters beyond convenience.** If the builder runs in the browser, a
redline can be composed, parsed, and diffed entirely client-side against shards
the page already has — no server in the loop, which is the same property that
makes the static site a full PII surface rather than a degraded one
(`living_corpus.md` §6). If it cannot, redline authoring needs a server and the
static-site surface is read-mostly. Tracked as **Issue 103 Part D**.

## 8. Relationship to git

Partly the same design, deliberately, and the divergences are the useful part.

Git stores blobs and trees and has **no rename records at all** — `-M` computes
similarity at read time, always fuzzy. The stub-shard/blob split is recognizably
tree/blob, and adopting a proven shape is a feature rather than an accident.

**First divergence: exact identity.** noet has an identity hash the compiler
already computed, so stage 2 resolves clean moves with no similarity pass. Git
cannot, because it has no semantic unit below the file. Fuzzy matching is noet's
fallback and git's only mechanism.

**Second divergence: the graph.** Git versions bytes; noet versions a graph, so
the archive must retain **relations**, which have no git analogue. This is why
the "just store blobs" instinct fails (§3.1).

**Third divergence: the dedup unit is the node, not the file.** Git's object
model bottoms out at the blob-per-file because it has no semantic unit below one.
noet does, and it has already computed them, so an edit re-stores one node body
rather than one file (§3.1).

**One thing worth copying exactly: whole objects, not deltas.** A git blob is the
complete content, zlib-compressed and content-addressed; delta chains live only
in packfiles, chosen by heuristic at `gc` time, depth-capped, and fully
reversible. Deltas are a *storage* layer beneath the object model rather than
part of it, which is what keeps checkout O(tree) instead of O(history).

The archive should hold that line. Storing a blob as `prior_hash + diff` would
make reconstruction O(chain), couple every blob to its predecessor's survival,
and break three properties this design leans on: eviction stops being free
(§9.3's cache-of-a-pure-function argument assumes any blob can go), stage 2 stops
resolving from stubs alone (§4), and a blob stops being self-verifying by its own
hash. If measurement later shows blob volume dominating, add packing **beneath**
the content-addressed store the way git does — the archive keeps saying "a blob
is a whole node body keyed by its hash", and a packer compresses that
representation invisibly.

## 9. Retention

Retention looks like one question and is two, and only one of them is this
document's.

**The corpus follows a source-control model**, and that is specified here:
generations are commits — named, promoted deliberately, and *re-derivable* for a
git-backed corpus (§9.1). **The annotation layer follows a save-game model** — a
reader wants a cursor and the trail to it rather than a milestone — and that is
**not** specified here, because an append-only log has nothing to discard
(§9.2). The two are different problems that happen to share a word.

They are joined at exactly one place: a record's `corpus_version` names the
generation it was written against. A trail through annotation time therefore
references a sequence of corpus states, which is what makes "what changed since I
read this?" answerable at all — and that join is §9.3, which *is* this
document's.

### 9.1 Corpus generations: commit, tag, HEAD

| Analogue | Here | Lifetime |
|---|---|---|
| working tree | the live graph | the session |
| `HEAD` / autosave | the `STAGED` generation, overwritten every parse | until the next parse |
| tag | a labelled generation, promoted by subcommand | until deliberately dropped |

**The default is a single overwritten `STAGED` generation.** It buys "working
tree versus previous parse" — the common interactive comparison — at bounded
cost. Interactive sessions must never accumulate generations; an editor loop
would otherwise archive on every keystroke.

**What promotes a generation is the inability to re-derive it.** If the codec
determinism contract holds (G1: node output is a pure function of content), a
git-backed corpus's prior graph is not data that must be retained — it is
recoverable by checking out the commit and re-parsing. There the archive is a
**cache of a pure function**: evictable at will, and garbage collection can cost
performance but never information. A **generated** corpus — one ingested from an
upstream system — cannot be re-derived, because nothing reconstructs what that
database held last Tuesday. There the archive is the only record.

So the rule falls out rather than being imposed: **bless what you cannot
re-derive.** For a git-backed corpus, blessing is an optimization; for a
generated one, it is the record.

### 9.2 Why the annotation layer needs no equivalent

The annotation layer has its own resumption problem — a reader wants the cursor
and the trail to it, not a milestone — but **it is not this document's**, because
one asymmetry removes it from the archive's concern entirely:

> The corpus archive stores **state** and must decide what to discard. The
> annotation log stores **events** and discards nothing, so its retention
> question is not "what do we keep?" but "which cuts are worth naming?"

Records are immutable and merge by set union, so the log *is* the trail:
replaying it to a cut reconstructs any prior annotation state exactly. Nothing
needs retaining because nothing is discarded. Naming a cut costs a label;
retaining a corpus generation costs a quarter of a shard set (§3.3), which is why
the expensive decision lives on this side and the cheap one does not.

**Trail UX belongs to the promotion follow-on** — what a cut is named, who names
one, whether an unnamed cut survives a session, and how a reader moves between
stores in a configured halo. Those are interactions among record stores, and the
mechanisms they compose are already specified elsewhere: a session's log ordered
by `EventId` and a **cut** across watermarks (`collector_model.md` §5.1, §5.2),
the regenerated scope (`living_corpus.md` §6), and promotion as a squash
(Issue 105 §Promotion reads the fold). This document owns only the join, below.

### 9.3 Where the two meet

A record carries the `corpus_version` it was written against — informational on
the record, load-bearing for the archive lookup, and **never the staleness
anchor** (`content_versioning.md` §6; the anchor is `(QuerySpec, tape_hash)`).

This is the join, and it is what makes a trail navigable rather than merely
stored. **A cut is a filter over corpus history**: given the generations its
records point into, "show me the corpus as it was across this trail" is a query,
not a reconstruction. The archive is what turns a list of `corpus_version`s into
graph states one can actually diff — `git log` scoped to the context an actor was
working in.

So retention on this side is not a correctness question. It has three tiers, and
they degrade rather than fail:

| The generation is | Then a trail resolves | Because |
|---|---|---|
| retained in the archive | immediately | the stub shards and blobs are there |
| evicted, corpus git-backed | after a re-derive | G1 makes the parse a pure function of content; check out the commit and rebuild |
| evicted, corpus generated | not at all | nothing reconstructs what the upstream system held |

**The middle row is the general resolution**, and it is why the pin-versus-degrade
fork was a false choice. A git-backed corpus never loses a generation
permanently: the archive is a *cache of a pure function* (§9.1), so eviction
costs latency, not information. Pinning is therefore unnecessary — it would
retain state that is recomputable by definition.

Two conditions this rests on, both already load-bearing elsewhere:

- **G1 and G5** — node output must be a pure function of content, comparable
  across binaries (`codec_determinism_contract.md`). Re-deriving a generation
  with a newer binary must produce the same hashes, or the recomputed state is
  not the state the records were written against. G5 exists precisely to detect
  when that stops being true.
- **BID stability** — re-parsing an old commit must reproduce the same BIDs, or
  the recovered generation cannot be joined to the records that reference it.
  This is Issue 66's `--hydrate-from` problem and Issue 74's stated Problem
  section; it is not solved by the archive and must hold independently.

**The third row is where deliberate retention is still required.** A generated
corpus — ingested from a database, an API, a spreadsheet export — has no commit
to re-derive from, so the archive is the only record and *bless what you cannot
re-derive* (§9.1) applies unchanged. This is the same rule, not an exception to
it: what varies is whether a generation is recoverable, and the corpus kind
decides that.

One consequence for the UI: a trail should report which tier each of its
generations is in. "Available", "recomputable", and "lost" are different answers,
and collapsing them would either hide work the reader can still recover or
promise recovery that cannot happen.

## 10. Open

- **Re-derive ergonomics** (§9.3). Recomputing an evicted generation is
  well-defined but not automatic: what triggers it, whether it is transparent to
  the reader, and where the rebuilt generation is cached are unspecified.
- **Stage 3's body source.** The residual set's bodies come from the live graph
  (within a run), the blob store (against an archived generation), or a parsed
  redline (§6). Same scoring, three fetch paths.
- **Whether a stage-3 hit emits one event or two** — `BidMigration` plus
  `NodeUpdate`, or a single migration carrying the content change.
- **Intra-node granularity.** Node-level structural diff is this document's
  subject; paragraph, cell, and property rendering is not. See §11.

## 11. The render is the harder half

Recorded because the format decision above is the easier one.

Structural diff answers *which nodes* changed. A reader wants to know *what*
changed, and reliable HTML rendering across property changes, structural moves,
paragraphs, tables, and image references is a genuinely hard problem. Three
things make it tractable rather than open-ended:

1. **Inline anchor nodes (Issue 91B, shipped)** already push BIDs below section
   granularity. Where authors use them, structural diff reaches paragraph level
   with no text differ at all.
2. **Tables and images are property diffs, not text diffs.** An image reference
   is a URL on a relation; a table is structured. Routing either through a text
   differ manufactures noise.
3. **A move must not re-render its body.** If the stub says `identity_hash` is
   unchanged, the node's HTML is byte-identical — the view says "moved" and
   stops. This is §3.2's payoff at the UI layer, and it is why the format choice
   reaches all the way into rendering cost.

## 12. References

- [`content_identity.md`](./content_identity.md) — `_identity_hash`, the
  link-collapse rule (§5), and why it is not `_content_hash`
- [`content_versioning.md`](./content_versioning.md) §5.1, §5.4 —
  `_content_hash` and the closure family
- [`../codecs/codec_determinism_contract.md`](../codecs/codec_determinism_contract.md)
  — G1, G2; why edges cannot be re-derived from stored bodies
- [`../annotation/overlay_model.md`](../annotation/overlay_model.md) §2.5, §2.6 —
  the candidate-state rule and the collision constraint behind it
- [`../annotation/living_corpus.md`](../annotation/living_corpus.md) §6, §11.2 —
  PII surfaces; the draft-anchor and diff-quality open questions
- [`../core/search_and_sharding.md`](../core/search_and_sharding.md) — the shard
  format this archive stubs
- `src/beliefbase/base.rs` — `compute_diff`
- `src/event.rs` — the vocabulary a move is expressed in
- `src/shard/search.rs:537,600,888` — `Stemmer`, `tokenize`, IDF scoring
