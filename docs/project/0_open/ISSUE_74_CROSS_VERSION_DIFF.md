# Issue 74: Cross-Version Structural Diff

**Priority**: MEDIUM
**Estimated Effort**: 7 days (RELATIVE COMPARISON ONLY)
**Dependencies**: Issue 66 (Incremental Parse — shard hydration mechanism). **For the archive and move-aware diff**: Issue 36's *move-detection* half — the archive stub carries `_identity_hash`, which Issue 36 computes and persists (see `docs/design/identity/generational_archive.md` §3.2). Issue 36's unification half is **not** a dependency and must not gate this issue. **For a browser-side redline diff**: Issue 103 Part D (`GraphBuilder` on wasm32).
**Design authority**: `docs/design/identity/generational_archive.md` — the archive format, the move-detection ladder, and the redline-as-file-map rule are specified there, not here.
**Supersedes**: Issue 73 (Versioned Rendering). Shard generations subsume per-version rendered output; a versioned render is recoverable on top of the archive, not the reverse.
**Version**: 0.1

## Summary

Enable structural comparison between two versioned BeliefBase snapshots using the existing query model primitives: a parameterized `ContentHash` NodeFilter for change detection, `And`/`Difference` compositions for set categorization, and progressive NodeFilter chains for property-level resolution. BID stability across builds is achieved via shard hydration into `global_bb` (Issue 66's mechanism). The viewer renders diff results as annotated traceability rows via a new `Diff` instrument render mode (query_model.md §7.3 extension).

No new projection primitives are introduced. Cross-version diff is a score interpretation that falls out of the existing algebra when the score carries content identity instead of relevance weight.

**Two consumers, and the snapshot case is only one of them.** The annotation
layer's live projection is a held-out BeliefBase
(`docs/design/annotation/living_corpus.md` §2), so comparison is that layer's
*native* read operation rather than machinery built for snapshots. Note the depth
difference: ordinary annotation is **additive** — the overlay only ever adds
nodes and edges — while a **redline** parses proposed content into a candidate
graph that may remove or reorder. The second is what needs a full diff. See
§The Annotation Layer below before fixing the design around versioned builds.

## Problem

Two independent `noet parse` invocations on the same source tree at different git tags produce BeliefBases with **different BIDs** for nodes whose BIDs were not persisted to source. BID generation embeds a timestamp (UUID v7), and ephemeral build directories don't carry forward prior BIDs. Without BID stability, cross-version comparison sees every node as "removed in A, added in B" — the join key is broken.

Nodes most vulnerable to BID instability:
- Nodes generated from external sources (API exports, codegen output)
- Spreadsheet rows before write-back has run
- Any source file where `noet parse --write` has not committed BIDs

## Goals

1. A `--hydrate-from <shard-dir>` CLI flag on `noet parse` that loads a previous build's shards as `global_bb`, providing BID continuity across independent builds.
2. A `ContentHash` NodeFilter (query_model.md §5.1) that hashes configurable node properties into `Score`, enabling change detection via score comparison.
3. Cross-version diff expressed entirely via existing compositions: `Difference` for added/removed, `And` + hash-score comparison for changed/unchanged.
4. Progressive property-level resolution via chained NodeFilters — coarse hash first, fine property diff second, noise filter third.
5. A `Diff` instrument render mode (§7.3) that presents all four categories (added, removed, changed, unchanged) in an annotated view.

## Architecture

### Core Insight: Diff as Score Interpretation

The query model's `Score = Option<f32>` semiring (§5.0) is the carrier. A content hash maps into it: two nodes with identical content produce identical scores; two nodes with different content produce different scores.

Given the same `QuerySpec Q` evaluated against two versioned graphs:

```
R_A = evaluate(Q + ContentHash, graph_A)   // Set<(Bid, Score=hash)>
R_B = evaluate(Q + ContentHash, graph_B)   // Set<(Bid, Score=hash)>
```

The four diff categories fall out of existing compositions:

| Category      | Expression              | Condition                        |
|---------------|-------------------------|----------------------------------|
| **Removed**   | `Difference(R_A, R_B)`  | BID in A, not in B               |
| **Added**     | `Difference(R_B, R_A)`  | BID in B, not in A               |
| **Unchanged** | `And(R_A, R_B)` where scores agree   | Same BID, same hash   |
| **Changed**   | `And(R_A, R_B)` where scores differ  | Same BID, different hash |

The unchanged/changed distinction is score equality within the `And` result. The `And` operator produces `min(s1, s2)` — but whether the two input scores **agree** is the diff signal. The instrument reads both pre-composition scores from the tape (§6) to make this determination.

No new projection primitive. No custom diff engine. The algebra handles it.

### `ContentHash` NodeFilter

A new named `NodeFilter` (§5.1) alongside `TextMatch`, `SchemaFilter`, etc.:

```
ContentHash(
    input: Set<Node>,
    include: Option<Vec<PropertyPath>>,   // hash only these (default: all)
    exclude: Option<Vec<PropertyPath>>,   // skip these (applied after include)
) -> Set<(Node, Score)>
```

The hash function is **parameterized** — different queries can hash different property subsets:

- **Content diff**: hash title + payload, exclude metadata → "did authored content change?"
- **Structural diff**: hash edge sets, exclude payload → "did graph structure change?"
- **Full diff**: hash everything → "did anything change at all?"

The exclusion set enables filtering out noisy properties (`metadata.git.checked_at`, `metadata.git.dirty`, timestamps) that change on every build but carry no semantic meaning.

Score value: `Some(fnv1a_32(selected_properties) as f32)`. Nodes always pass the filter (the hash is always `Some`) — `ContentHash` is a **score annotator**, not a down-selector.

### Progressive Resolution via Chained Filters

Option (b) from the design exploration: the `And` result set preserving both input scores enables a coarse-to-fine pipeline.

```
// Stage 1: Content hash — O(1) per node
//   Separates "definitely unchanged" from "possibly changed"
R_both = And(R_A, R_B)
maybe_changed = NodeFilter(|n| hash_a[n.bid] != hash_b[n.bid])

// Stage 2: Property diff — O(fields) per node, only on maybe_changed set
//   Score = count of changed fields (or bitmask, or weighted sum)
property_diff = NodeFilter(|n| {
    diffs = compare_fields(node_a[n.bid], node_b[n.bid], exclude)
    if diffs.is_empty() { None }   // hash collision — actually unchanged
    else { Some(diffs.len() as f32) }
})

// Stage 3: Noise filter — policy decision, optional
//   Filter out nodes where only "uninteresting" properties changed
significant = NodeFilter(|n| property_diff_score > threshold)
```

Each stage is a standard `NodeFilter` in a chain. Each narrows the set and refines the score. Stage 1 is cheap, Stage 2 is expensive but runs on a smaller set, Stage 3 is configurable policy. This is exactly how the projection chain is designed to work.

The property-diff stage can itself output a score encoding *which* properties changed — enabling downstream filtering on specific change types. Symmetrically, the `ContentHash` stage's `exclude` parameter means imperatively-set or derived properties can be ignored from the outset, preventing them from polluting the "changed" signal.

### BID Stabilization via `--hydrate-from`

`--hydrate-from` is a subset of Issue 66's full incremental parse. It hydrates `global_bb` from a previous shard directory but always re-parses all source files (no mtime-based skipping). The sole purpose is BID resolution continuity.

```
noet parse --hydrate-from previous_output/beliefbase/ \
           --html-output output/ src/
```

Behavior:
1. Read `<shard-dir>/manifest.json`
2. Deserialize network shards into the in-memory `global_bb` (`BeliefBase`)
3. Parse all source files normally
4. `GraphBuilder` resolves BIDs against `global_bb` using the existing resolution hierarchy:
   - Explicit BID in source frontmatter → use it (highest priority)
   - Path or ID match in `global_bb` → reuse `global_bb`'s BID
   - No match → generate fresh BID (existing fallback)
5. Export shards — BIDs stable for all nodes that existed in the hydration source

This reuses the BID resolution hierarchy already implemented for `noet watch`. The difference: `noet watch` populates `global_bb` from its live DB; `--hydrate-from` populates it from serialized shards.

**Shard format compatibility**: incompatible shard format → warning + empty `BeliefBase` fallback. Never fail the build due to stale shards.

**Partial hydration**: previous shards cover only a subset of networks → uncovered networks get fresh BIDs. Correct behavior.

### Diff Instrument Render Mode

Extend query_model.md §7.3 with a `Diff` render mode alongside `Table`, `Graph`, `Scalar`:

The `Diff` instrument:
1. Evaluates the query against both current and comparison graphs
2. Applies `ContentHash` + `And`/`Difference` compositions
3. Optionally runs progressive property-diff NodeFilters
4. Annotates each result row with its diff category
5. Renders the annotated union of both result sets

The `InstrumentConfig` gains:

```
diff_source: Option<VersionedBeliefBaseRef>   // comparison version
diff_hash_exclude: Vec<PropertyPath>          // noisy properties to ignore
```

When `diff_source` is absent, the instrument operates normally (single-version mode, backward-compatible).

### Viewer Integration

The traceability table (Issue 63) renders diff annotations as row decorations:
- 🟢 **Added**: row in current version but not comparison
- 🔴 **Removed**: ghost row — in comparison but not current
- 🟡 **Changed**: in both, content hash differs
- ⚪ **Unchanged**: in both, content hash agrees

The version selector (Issue 73) gains a "Compare with..." option that loads a second version's sharded BeliefBase and activates diff mode.

## Implementation Steps

### Phase 1: `--hydrate-from` CLI flag (2 days)

1. **Shard deserialization into `BeliefBase`** — `src/shard/` (1 day)
   - [ ] Add `hydrate_beliefbase_from_shards(shard_dir: &Path) -> Result<BeliefBase>` that reads `manifest.json`, deserializes network shards, inserts nodes/edges into a `BeliefBase`
   - [ ] Handle shard format version mismatch: warn, return empty `BeliefBase`
   - [ ] Handle partial manifests: hydrate available networks, skip missing

2. **CLI integration** — `src/bin/noet/main.rs`, `src/codec/compiler.rs` (1 day)
   - [ ] Add `--hydrate-from <shard-dir>` optional argument to `Parse` command
   - [ ] When present, call `hydrate_beliefbase_from_shards` and pass result as `global_bb` to `DocumentCompiler`
   - [ ] Integration test: parse source → export shards → parse again with `--hydrate-from` → BIDs match

### Phase 2: `ContentHash` NodeFilter (1.5 days)

3. **ContentHash implementation** — `src/query.rs` or `src/query/filters.rs` (1 day)
   - [ ] Define `ContentHash { include: Option<Vec<String>>, exclude: Option<Vec<String>> }` as a `NodeFilter` variant
   - [ ] Implement hash computation: FNV-1a over selected `title + payload + content` fields
   - [ ] `exclude` filters out specified property paths before hashing
   - [ ] Score output: `Some(hash as f32)` — always passes, never filters
   - [ ] Unit tests: identical nodes produce identical scores; different nodes produce different scores; excluded fields don't affect hash

4. **Cross-version composition helpers** — `src/query.rs` (0.5 day)
   - [ ] Helper function: `diff_sets(r_a, r_b) -> (added, removed, changed, unchanged)` using `Difference` + `And` + score comparison
   - [ ] This is a convenience wrapper, not a new primitive — implemented in terms of existing `SetOp`
   - [ ] Unit tests: all four categories correctly identified from two scored result sets

### Phase 3: Diff instrument mode and viewer (2.5 days)

5. **Instrument extension** — `src/shard/wasm.rs`, instrument layer (1 day)
   - [ ] Add `diff_source: Option<ShardRef>` to instrument configuration
   - [ ] When set, evaluate query against both graphs, apply `diff_sets`, annotate results
   - [ ] WASM export: `diff_query(query, current_shards, comparison_shards) -> JsValue`
   - [ ] Serialize diff annotations alongside normal query results

6. **Viewer diff mode** — `assets/viewer/` (1.5 days)
   - [ ] "Compare with..." option in version selector dropdown (requires Issue 73)
   - [ ] Load comparison version's sharded BeliefBase into second WASM instance
   - [ ] Call `diff_query` and annotate traceability table rows with CSS classes
   - [ ] Diff summary badge: "N added, M removed, K changed"
   - [ ] Toggle to exit diff mode

### Phase 4: Documentation (1 day)

7. **Design doc updates** — `docs/design/core/query_model.md` (0.5 day)
   - [ ] §5.1: document `ContentHash` NodeFilter with `include`/`exclude` parameters
   - [ ] §7.3: document `Diff` render mode
   - [ ] §4: note that versioned graph snapshots are valid `BeliefGraph` references
   - [ ] §11: add open question about content hash precision (f32 collision rate)

8. **User documentation** (0.5 day)
   - [ ] Document `--hydrate-from` flag and BID stabilization workflow
   - [ ] Document "Compare with..." viewer workflow
   - [ ] Document CI cache pattern for shard-based BID continuity

## Testing Requirements

### Unit Tests
- `hydrate_beliefbase_from_shards`: correct node/edge count from test fixtures; handles missing/corrupt shards
- BID stability: parse → export → parse with `--hydrate-from` → BIDs match for all nodes
- `ContentHash`: identical nodes produce identical scores; excluded properties don't affect hash; different content produces different hash
- `diff_sets`: correctly categorizes added/removed/changed/unchanged from two scored sets
- Progressive resolution: Stage 2 NodeFilter correctly identifies changed properties on the maybe-changed subset

### Integration Tests
- Full round-trip: build v1 → modify source → build v2 with `--hydrate-from v1` → diff → changed nodes reflect actual source changes
- Shard format mismatch: hydration from incompatible shards produces warning and fresh BIDs, not error
- Single-version builds (no `--hydrate-from`, no diff mode): identical output to pre-issue behavior

### Manual Tests
- "Compare with..." in viewer shows correct diff annotations
- Diff mode toggle works (enter/exit)
- Diff summary badge shows accurate counts
- Excluded properties (e.g., `metadata.git.checked_at`) don't cause false "changed" signals

## Success Criteria

- [ ] `noet parse --hydrate-from prev/beliefbase/ --html-output curr/ src/` produces BIDs consistent with `prev/` for all nodes that exist in both versions
- [ ] `ContentHash` NodeFilter produces identical scores for identical node content and different scores for different content
- [ ] `diff_sets` correctly categorizes nodes into added/removed/changed/unchanged with no false positives on unchanged nodes
- [ ] Progressive resolution chain narrows the "changed" set and identifies specific changed properties
- [ ] Traceability table in diff mode shows row-level annotations with correct visual indicators
- [ ] No new projection primitives introduced — diff is expressed via existing `NodeFilter`, `And`, `Difference`

## Risks

- **f32 hash collision**: FNV-1a truncated to f32 has ~4 billion distinct values. For corpora under 100k nodes, collision probability is negligible (~0.001%). For million-node corpora, consider f64 or a dual-hash scheme. → **Mitigation**: start with f32; the progressive resolution Stage 2 catches collisions (hash agrees but properties differ).

- **Memory doubling in diff mode**: loading two BeliefBases in the WASM viewer. → **Mitigation**: load only overlapping networks (compare manifests); for large corpora, compute diff server-side via MCP. Start with client-side for small/medium corpora.

- **BID drift across branches**: two branches independently stabilize BIDs via separate shard caches. → **Mitigation**: explicit BID in source (write-back) always wins; for ephemeral BIDs, last-writer-wins at merge time. Existing concern, not new.

- **Noisy property exclusion**: if the `exclude` set is wrong, legitimate changes are hidden. → **Mitigation**: the `exclude` parameter is per-query, not global. Default is empty (hash everything). Users opt into exclusion explicitly.

## Open Questions

1. **`--hydrate-from` as Issue 66 subset**: implement independently or as Issue 66's first deliverable? → **Recommendation**: independently — needs only shard deserialization, not mtime-skip logic. Issue 66 builds on the same path later.

2. **Hash precision**: f32 sufficient, or use a separate hash field outside `Score`? → **Recommendation**: f32 for initial implementation. Progressive resolution Stage 2 catches collisions. Revisit if corpus sizes exceed 100k nodes.

3. **Tape preservation of pre-composition scores**: `And(R_A, R_B)` currently produces `min(s1, s2)`. The diff instrument needs both input scores to determine agreement. Should the tape (§6) preserve pre-composition scores, or should the instrument re-evaluate? → **Recommendation**: the tape already records per-step results. The instrument reads the per-step entries for `R_A` and `R_B` independently, then compares. No tape schema change needed.

4. **Edge diff as separate hash**: should structural (edge) changes use a separate `ContentHash` that hashes edge sets, or fold edges into the node-level hash? → **Recommendation**: separate hash stage. Content changes and structural changes are different concerns — a node can have unchanged content but new edges (e.g., new coverage claim). Two `ContentHash` filters in sequence, each hashing different property subsets, is the clean decomposition.

5. **Score encoding for property-diff stage**: should the Stage 2 score encode *which* properties changed (bitmask) or just *how many* (count)? → **Recommendation**: count for initial implementation. Bitmask is a future refinement if downstream filters need to branch on specific property types.

## Design Note: Why Not a Custom Diff Engine?

An earlier draft proposed a `diff.rs` module with `VersionDiff`, `NodeDiff`, and `DiffAnnotation` data structures. This was rejected in favor of expressing diff via existing query model primitives because:

1. **No new algebra**: `ContentHash` is a `NodeFilter`. Added/removed are `Difference`. Changed/unchanged are `And` + score comparison. The existing composition operators handle all four categories.

2. **Progressive resolution is free**: chained `NodeFilter` stages provide coarse-to-fine diff at no architectural cost. A custom engine would need to reimplement this pipeline.

3. **Property exclusion is free**: `ContentHash`'s `exclude` parameter handles noisy properties. A custom engine would need its own exclusion mechanism.

4. **Composability**: because diff is expressed in the query algebra, it composes with all other projection steps. You can diff a filtered subset, diff a traversal result, or diff a composed query. A custom engine would be a parallel, non-composable code path.

The instrument layer (§7) is the only genuinely new code: a `Diff` render mode that presents the union of two evaluated result sets with per-row annotations.

## The Annotation Layer: Diff as the Native Operation

This issue is *written* around comparing two versioned snapshots, but that is not
the only — or the primary — consumer. Per `docs/design/annotation/living_corpus.md` §2,
**the annotation layer's live projection is a held-out BeliefBase, in essence a
diff applied to its root corpus context.** The annotation server maintains that
overlay and merges it continuously on top of the compiled graph.

So for the annotation layer, comparison is not an application of machinery built
elsewhere — it *is* the layer's read operation. "Compiled corpus" versus "corpus
with the annotation queue folded in" is not a comparison performed on the
overlay; it is what looking at the overlay means.

**But the two consumers exercise different depths of it.** Projection is
additive: an annotation owns edges into the nodes it concerns and modifies none
of them, so the delta is purely new material and the "diff" is the halo. A
redline's payload parses into a **candidate graph** that can mutate and remove,
which is where `Difference` over content identity does real work. Build for the
second and the first falls out; build only for the first and redlines have no
surface. This issue builds the primitive
that operation is expressed in, and should be evaluated against that use as much
as against snapshot comparison.

Mechanically it is the same computation either way: a `ContentHash`-scored
comparison over two graph states, with added / removed / changed / unchanged
falling out of `Difference` and `And`. The `Diff` render mode remains the natural
surface — for the annotation case, it shows a pending edit before it is
committed.

The versioned-snapshot case is unaffected and remains in scope: comparing two
builds of a corpus is a real need with its own BID-stability problem
(§Problem, `--hydrate-from`). It is simply no longer the framing that governs the
annotation half.

### A third consumer: the change package

A **change package** is what crosses the organizational boundary when a redline
cannot be enacted directly — which is the common case, since most targets are
controlled documents or repositories noet cannot write to
(`docs/design/annotation/living_corpus.md` §7). It carries proposed text against
current text, the rationale, the requirement driving the change, and the impact
set.

Structurally that is **a diff with provenance attached**, and the diff half is
this issue's machinery pointed at a *proposed* rather than an *observed* change:
one side is the corpus as it is, the other is the corpus with a redline's
replacement folded in. The provenance half the redline record already carries.

Two things follow. The package is a **rendering**, not a new subsystem — no
bespoke change-request builder is needed. And it is **useful with no write
authority at all**, which decouples it from Issue 106/107: the render is what a
human carries to a document-control process, and it is valuable precisely where
enactment is impossible. Note the direction — the package is produced wholly on
the *read* side; no source text is written to produce it.

#### The unowned step: what is the second side of the comparison?

This issue diffs two graph states. A change package diffs the corpus against
**the corpus as it would be if a redline landed**, and nothing currently owns
producing that second state. Two shapes, and they are not equivalent:

| | **Two-payload** | **Candidate graph state** |
|---|---|---|
| What is compared | the redline's proposed text vs. the target node's current text | base graph vs. graph with the redline folded in |
| Needs a graph? | no | yes |
| Serves | a single-target redline | a redline whose landing changes several nodes |
| Coverage of observed data | 8 of 25 hand-written records | all 25 |

The second is required by the real case: one change plan landing flips ten
coverage tags *and* adds cross-reference blocks citing sections that did not
exist when the gap was written. No two-payload view shows that.

> **This fork is now settled — the candidate state is required, not preferred.**
> `docs/design/identity/generational_archive.md` §6 shows the
> two-payload form is not merely weaker but *insufficient*: proposed text that
> has never been parsed carries no `_content_hash` and no `_identity_hash`, so it
> produces no node comparable with the corpus and structurally cannot answer "did
> this section move." §6.1 fixes the redline's shape as a map of corpus-relative
> path to content, parsed by the real `GraphBuilder` — the file being the
> smallest unit at which a parse is total and an anchor therefore well-defined.

> **The candidate side must resolve to the corpus's identities, and must never be
> stored.** Diff pairs nodes by BID — which is why `--hydrate-from` exists — but
> the candidate need not have BIDs assigned up front: parsing proposed source
> resolves each node's key list against the existing graph (`cache_fetch`,
> `src/codec/builder.rs:4026`), so a node may pair by `Path` or `Id` and carry the
> corpus BID once paired. Pairing on any `NodeKey` variant rather than BID alone
> is what lets a retitled section read as an edit instead of a delete plus an add.
> Reusing the base's BIDs is safe
> **only** because it is a rendering artifact: it lives in one `QueryPackage`,
> is discarded after the read, and is never merged back. Persisting it would put
> two accounts of one node's content in a store with nothing to reconcile them,
> and `BeliefGraph::union_mut` (`src/beliefbase/graph.rs:706-714`) would resolve
> the collision by destroying the base node.
>
> `docs/design/annotation/overlay_model.md` §2.5 states the rule and §2.6 the
> constraint behind it. Nothing in the type system distinguishes a safe candidate
> from an unsafe one, so this issue should assert it in a test: **a candidate
> package is not reachable from any `BeliefSource` after the render completes.**

Recorded here, as with the annotation case, so the instrument's output shape does
not foreclose it — not as a dependency in either direction.

Two implications worth carrying while building this:

- **BID stabilization matters differently.** §Problem solves BID instability
  across independent builds via `--hydrate-from`. In the annotation case both
  sides derive from the *same* live graph, so BIDs are stable by construction —
  except for draft content, which has no BID at all. Do not assume both sides of
  a diff always have BIDs on every node.
- **The edit script may be consumed, not only displayed.** Rendering a diff needs
  only correctness. Emitting one as `BeliefEvent`s — which is what committing an
  annotation queue means — additionally wants the *most semantically meaningful*
  sequence of primitive operations: "moved this section" rather than "removed
  here, added there." **This issue owns that.** Telling a reorder apart from a
  delete-plus-add, and an edit apart from a replacement, is what a structural
  diff is for; a comparison that cannot do it reports churn instead of change.
  Draft content sharpens the same requirement — a draft may *supplant* an
  existing node's position, so the script must express a move
  (`PathUpdate` carries an order vector) rather than a removal and an addition
  (`living_corpus.md` §5).

Not a dependency in either direction; recorded so the machinery is not built in a
way that serves only the snapshot case.

## Creating an Archive: making "what changed" answerable

**Scope: after the W4 pilot, prioritized by use.** The MVP can say a receipt is
stale; it cannot say *what* moved. That is `git status` without `git diff` — the
reader must re-read the document, which is most of the cost the receipt exists
to remove. This section records the design so the MVP does not foreclose it.

### The store: stub shards plus a blob cache

> **Specified in `docs/design/identity/generational_archive.md` §3.**
> The format is a stub shard — `BTreeMap<Bid, (ContentHash, IdentityHash,
> BeliefKindSet)>` plus whole relations — alongside a content-addressed blob
> store. §3.2 explains why the stub carries `identity_hash` (without it a moved
> section reads as remove-plus-add); §3.3 gives the measured ~25% cost; §3.4
> lists the two checks before the format is frozen.
>
> **Move detection is a three-stage ladder** (§4): naive diff, exact
> `identity_hash` match, then TF-IDF over the remainder using the shipped
> `tokenize`/`Stemmer`/IDF machinery. Stages 2 and 3 belong to
> `DocumentCompiler`, not `GraphBuilder` (§5) — a move is a cross-file
> observation and the builder sees one file at a time. Issue 36 owns the
> detection implementation; this issue consumes it.

This issue's share of the archive is the **read path and the render**:

### The read

1. Receipt's `corpus_version` → that generation's manifest → the stub shards
   covering its scope
2. Stubs → blobs for the node bodies
3. Assemble `BeliefGraph` → `BeliefBase::from` (`src/beliefbase/base.rs:147`) —
   the existing hydration path, not a new one
4. `BeliefBase::compute_diff(old, new, scope)` (`:704`) yields the ordered delta,
   which is this issue's render input

**This is a better `--hydrate-from`.** That flag needs a whole prior shard set on
disk; the archive rehydrates only the networks a receipt's scope touches.
Cross-version diff between two builds still wants the flag; diff against *what I
read* wants the archive.

### Three-way diff: the archive supplies a merge base

A redline records the `corpus_version` it was written against. When the corpus
has moved past it, the archive resolves that generation and the comparison
becomes three-way rather than two-way — base, current, proposed. That is what
separates *the author changed this* from *the corpus changed underneath them*,
and the two need opposite handling
(`identity/generational_archive.md` §6.4 has the classification table).

Two consequences for this issue:

- **The `Diff` render mode needs a three-side variant**, or at least must not
  assume two. A conflict — both sides changed the same span — is a distinct
  render state from *changed*, and collapsing them loses the distinction that
  makes staleness actionable.
- **It degrades cleanly.** With no base generation available the comparison is
  the ordinary two-way one, still correct and still renderable; what is lost is
  conflict detection. Build the two-way case first and treat the base as an
  optional third input.

### Retention

> **Specified in `docs/design/identity/generational_archive.md` §9.** The corpus
> follows a source-control model: `STAGED` overwritten by default, labelled
> generations promoted deliberately, on the rule *bless what you cannot
> re-derive* (§9.1). The annotation layer needs no equivalent — an append-only log
> discards nothing, so a trail is replayed rather than retained, and its
> interaction layer is an Issue 105 follow-on (§9.2). §9.3 covers the join: a
> record's `corpus_version` points at a generation, so a trail is only as
> replayable as the generations its records reference.

### Yes, this is git's data model

Worth naming, because the resemblance is a design signal in both directions.

| git | here |
|---|---|
| blob | node body, content-addressed |
| tree | stub shard (names + hashes + structure) |
| commit | a generation |
| tag | a blessed label |
| index / working tree | `STAGED` |
| reachability GC from refs | prunable unless a live receipt points in |
| `diff A B` | `compute_diff(old, new)` |

Arriving at it independently is evidence the constraints are real. It is also a
warning: git spent two decades on packing, delta compression, and GC, and a naïve
version of each is where storage goes wrong. **Using git itself** was considered —
`beliefbase/archive/` as a nested repo, the same argument the record store makes
for its own sidecar — and rejected for one hard reason: **the browser cannot run
git**, and the halo query is meant to work client-side against a static site.
Msgpack's poor delta compression is a soft second reason.

### The ladder

Three rungs, each useful alone. Do not build rung 2 before rung 1 has users.

1. **Per-node text diff from a blob cache.** For the common case — prose changed
   in a section I read — what a reader needs is old `payload.text` against new.
   That is one blob and a string diff rendered through `render_markdown_snippet`,
   which the halo already uses. No relations, no hydration, no `compute_diff`, no
   generations.
2. **Stub-shard generations** for blessed builds, when graph-level diff
   (containment, edges, ordering) is wanted.
3. **Packing** if generations proliferate enough to need it.

### Two properties that fall out

- **GC has a derived criterion.** A generation is prunable when no live receipt
  points into it; blobs are prunable when no retained stub shard names them.
  Retention is *defined by the receipts* rather than by a separate policy.
- **Relation diffs come from stored relations, not re-derivation.** The stub
  shard carries the edges, so nothing has to reconstruct them. Node-level
  containment still follows the "include the parent" rule (below), but that is a
  scoping question now, not a derivability one.

### What the MVP must not foreclose

Three constraints, cheap now and expensive later:

- **Issue 104**: a receipt's `tape_hash` must be resolvable to its member set —
  either a tree object in the archive or a manifest in the receipt payload.
  Decide at census time; the archive prefers the tree, since receipts share it.
- **Issue 103 Part B**: the canonical serialization used to compute
  `content_hash` is the serialization the blob stores. One form, not two.
- **Issue 66**: the export path holds the prior node when it computes the new
  hash. Do not structure it so the prior is dropped before the new is written.

### Open

- Whether the archive is one store per corpus or rides the annotation store
  halo (`annotation_channel.md` §6). It is compiler output, not an annotation,
  which argues for the former.
- **Whether a stub carries `BeliefKindSet`** — see the format checks above. This
  is the one open question that changes the size estimate.
- Whether a stub shard preserves **tape order** for the scope a receipt covered,
  or only the network's own ordering. `tape_hash` sorts member hashes lexically
  by design (`content_versioning.md` §4.3), so order cannot come from the anchor.
  Within a network, relations carry `WEIGHT_SORT_KEY`, so the stub shard has it;
  the gap is only for a scope spanning networks.
- Whether hydration should **close containment automatically** — pull in a
  member's parent network or document even when the receipt's scope excluded it.
  It makes a generation self-sufficient for diffing at the cost of retaining
  shards nobody asked about. A receipt anchored to `id://doc composed_of(*)`
  already includes its document, so this only bites on hand-written scopes.

## References

- `docs/design/annotation/living_corpus.md` §2 — the held-out-BeliefBase model of Layer 3's
  live projection; authoritative for why diff is the annotation layer's native
  operation
- `docs/design/annotation/living_corpus.md` §2 — projection then diff; the annotation-queue case
- Issue 66: Incremental Parse via Shard Hydration — shard deserialization, `global_bb` hydration
- Issue 73: Versioned Rendering — per-version sharded output, version selector UI
- Issue 63: Traceability View (COMPLETE) — primary rendering surface for diff annotations
- Issue 70: Unified Search, Query, and Graph Visualization UI — future diff integration
- `docs/design/core/query_model.md` §5.0 (Score), §5.1 (NodeFilter), §5.3 (Compositions), §7 (Instrument)
- `docs/design/core/beliefbase_architecture.md` §2.2 — BID resolution hierarchy
- `src/shard/manifest.rs` — `NetworkShardMeta`, `ShardManifest`
- `src/shard/wire.rs` — `NetworkShard`, `GlobalShard` (deserialization types)
- `src/codec/compiler.rs` — `DocumentCompiler`, `global_bb` threading
- `.scratchpad/cross_version_query_exploration.md` — design exploration