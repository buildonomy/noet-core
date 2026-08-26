# Issue 101: Investigate Replacing BTreeMap/BTreeSet with HashMap/HashSet in BeliefBase

**Priority**: MEDIUM
**Estimated Effort**: 2-3 days (RELATIVE COMPARISON ONLY) — investigation +
prototype; a full migration (if warranted) would be its own follow-on issue.
**Dependencies**: None directly, but shares motivation with Issue 99
(large-corpus performance) — this is a candidate root-cause lever for
general throughput, independent of that issue's specific stall findings.

## Summary

`BeliefBase` and its supporting types (`BidGraph`, query evaluation,
diffing) use `BTreeMap<Bid, _>` / `BTreeSet<Bid>` pervasively — for the
primary `states` map, the `bid_to_index` lookup table, and a large number of
ad-hoc traversal-frontier and result-set collections throughout
`compute_diff`, `apply_traversal_to_tape`, `apply_projection_steps_to_package`,
`insert_state`, `remove_nodes`, and more (see `src/beliefbase/base.rs`,
`src/beliefbase/graph.rs`).

The original motivation for BTree was believed to be a need for **stable,
deterministic key ordering** — but on reflection, this may no longer (or
never really) be the actual requirement:

1. `Bid`s are orderable post-hoc (sorting a `Vec<Bid>` collected from a
   `HashMap` is trivially available whenever deterministic *output* order is
   actually needed — e.g. for snapshot tests or diff display).
2. The ordering that structurally matters — the position of an edge among
   its siblings — is captured explicitly by `WEIGHT_SORT_KEY` on
   `BeliefRelation`/`Weight`, not by the iteration order of the containing
   map/set. Code that needs sibling order already sorts explicitly by
   `WEIGHT_SORT_KEY` at the point of use (e.g.
   `apply_traversal_to_tape`'s hop-edge sort, `reindex_sink_edges`).

If (1) and (2) hold generally across the codebase, `BTreeMap`/`BTreeSet`
keyed on `Bid` could be replaced with `HashMap`/`HashSet` using a fast,
non-cryptographic hasher (e.g. `rustc-hash::FxHashMap`/`FxHashSet` or
`ahash`) for a potential CPU/allocation win on the hot paths that
`compute_diff`, `insert_state`, and query traversal all sit on. DoS
resistance (the usual reason `std::collections::HashMap`'s default
`SipHash` exists) is not a concern here — `noet-core` is an embedded
library, not a network-facing service processing untrusted input at this
layer.

## Goals

1. **Audit every BTree usage** in `src/beliefbase/*.rs` (and any other
   module leaning on `BeliefBase`'s ordering, e.g. `src/paths/pathmap.rs`,
   `src/query/*.rs`, `src/shard/*.rs`) and classify each as:
   - **(a) Pure lookup/membership** — no order dependency at all; trivially
     safe to swap.
   - **(b) Iteration order affects an observable output** — e.g. tests that
     assert on `Vec` collected from iterating the map/set, deterministic
     JSON/msgpack export, HTML generation order, or diff/warning message
     ordering that users or snapshot tests depend on. These need explicit
     sorting inserted at the point of output if switched to HashMap.
   - **(c) Genuinely relies on BTree's sorted-iteration semantics for
     correctness** (not just determinism) — e.g. any algorithm that assumes
     processing BIDs in sorted order for a correctness reason, not just
     reproducibility. This category should be small or empty per the
     WEIGHT_SORT_KEY argument above, but must be verified, not assumed.

   **STATUS: DONE.** See "Audit Findings" section below for the full
   classification across `base.rs`, `graph.rs`, `pathmap.rs`, `spec.rs`,
   and `shard/{export,search}.rs`. Headline result: **zero category (c)
   findings** across all five files/modules audited. The WEIGHT_SORT_KEY /
   explicit-sort-at-output-boundary hypothesis holds everywhere it was
   tested.
2. Determine whether `BidGraph`'s underlying `petgraph` graph (`StableGraph`
   or similar) has any BTree dependency of its own that would need parallel
   treatment, or whether it's already using index-based (non-ordered)
   storage internally.

   **STATUS: DONE.** Confirmed via direct read of petgraph 0.6.5 source:
   `StableGraph` (backing `BidGraph`) is `Vec`+free-list-backed;
   `GraphMap` (backing `BidSubGraph`) is `IndexMap`-backed. Neither has
   any BTree dependency. Not a blocker.
3. Prototype the swap for the highest-traffic structures first — `states:
   BTreeMap<Bid, BeliefNode>` and `bid_to_index: BTreeMap<Bid, NodeIndex>`
   — using a fast hasher, and benchmark against the existing Criterion
   benchmarks (`benches/document_processing.rs`,
   `benches/macro_benchmarks.rs`) plus a real corpus run (see Issue 99 for
   the large-corpus scenario) to quantify actual wall-clock/CPU impact.

   **STATUS: DONE.** `bid_to_index` swapped to `FxHashMap<Bid, NodeIndex>`
   (commit `8a48a96`) and validated via `benches/document_processing.rs`:
   the lookup-only `graph_queries` benchmark (isolated from I/O noise)
   improved ~40-43%, reproduced across runs. `states` (on both
   `BeliefBase` and `BeliefGraph`) subsequently swapped to
   `FxHashMap<Bid, BeliefNode>` as well — see "Scope Decision: `states`"
   below for the full migration writeup (public-API/serialization-order
   concerns were reassessed and cleared, not deferred).
4. Fix any test or output-determinism breakage surfaced by category (b)
   above, by sorting explicitly at the boundary rather than relying on
   incidental map ordering.

   **STATUS: DONE.** The `states` migration surfaced exactly the
   predicted category-(b) cases: three tests doing `.keys().collect::<Vec<_>>()`
   equality between independently-built maps (relies on both producing the
   same order, which `HashMap` doesn't guarantee) were fixed to compare as
   `BTreeSet`s instead; two tests picking `network_nodes[0]` after a
   `.values().filter()` scan (relying on incidental Bid-sort order to find
   "the" test network among several synthetic ones) were fixed to select
   by title explicitly. All fixes are in `src/beliefbase/graph.rs` and
   `src/codec/compiler.rs`; see git history for exact diffs.
5. Produce a clear recommendation: proceed with a full migration, proceed
   with a partial migration (e.g. only the hottest structures), or document
   why BTree should stay (e.g. if the audit finds pervasive, hard-to-remove
   category (c) dependencies).

   **STATUS: DONE.** See "Recommendation" section at the end of this
   document.

## Architecture Notes / Where to Start

- `src/beliefbase/base.rs` — `BeliefBase.states: BTreeMap<Bid, BeliefNode>`,
  `BeliefBase.bid_to_index: RwLock<BTreeMap<Bid, NodeIndex>>` (note: this one
  is `#[cfg]`-gated with a `RefCell` variant for wasm32 — any change must
  handle both). Also every ad-hoc `BTreeSet<Bid>`/`BTreeMap<Bid, _>` used as
  a traversal frontier, result accumulator, or dedup set throughout
  `compute_diff`, `insert_state`, `remove_nodes`, `apply_traversal_to_tape`,
  `apply_projection_steps_to_package`, `merge_graph_mut`, `trim`.
- `src/beliefbase/graph.rs` — `BidGraph` and whether its petgraph backing
  store has independent ordering assumptions.
- `src/query/spec.rs` — `Tape`, `QueryPackage`, and other query-evaluation
  structures that may collect `BTreeSet<Bid>` results.
- `src/paths/pathmap.rs` — **Original assumption REFUTED by audit.** The
  hypothesis that `PathMap`/`PathMapMap` need genuine sorted order for
  document/section *position* does not hold: zero `.range()` calls exist
  anywhere in the file, and every `BTreeMap`/`BTreeSet` access (`bid_map`,
  `path_map`, `order_map`, `nets`, `docs`, `apis`, `stubs`, `titles`,
  `ids`, `node_to_nets`) is an exact-key point lookup or pure membership
  check. The actual position/order source of truth is
  `PathMap.map: Vec<(String, Bid, Vec<u16>)>`, kept sorted via an
  explicit `.sort_by(pathmap_order)` call — i.e. the same
  explicit-sort-of-a-Vec pattern this issue recommends generally, already
  in place. `order_map`'s doc comment claims "O(log N) ancestor lookup by
  order prefix" but tracing `order_for()`'s only caller
  (`update_path_segment`) shows it's a single exact lookup on one
  pre-sliced prefix, not a walked range scan — the doc comment overstates
  what the code does and should be corrected. **`pathmap.rs` is IN SCOPE**
  for a future migration; see "Audit Findings" below for the full
  classification. `PathMap.path_map` (confirmed dead — no live reader
  outside a test helper) has since been **removed** (see below). One
  remaining adjacent finding, tracked separately, not a blocker:
  `PathMap.subnets: BTreeSet<Bid>` has a
  pre-existing, container-independent multi-match tie-break ambiguity that
  swapping to `HashSet` doesn't worsen but also doesn't fix.)
- Consider `indexmap::IndexMap`/`IndexSet` as a middle-ground option for any
  structure in category (b) where insertion-order (not sorted-order)
  determinism is actually what's needed — this can avoid re-sorting at
  output time while still dropping the BTree comparison overhead.

## Audit Findings (Goal 1, completed)

Full per-location classification tables live in session notes; this section
summarizes the conclusions per file/module. "(a)/(b)/(c)" per the Goals
section definitions.

### `src/beliefbase/base.rs` + `src/beliefbase/graph.rs`

~30 (a) findings, ~7 (b) findings, **zero (c)**. Every genuinely
order-sensitive output (traversal hop edges, reindexed sink edges,
`to_event_stream` edge emission) is governed by an explicit `sort_by`/
`sort_by_key` on `WEIGHT_SORT_KEY` or tape-recorded order — never by raw
map/set iteration. Notable (b) findings needing an explicit `.sort()` if
migrated:

- `compute_diff`'s Phase 3/5 event-push loops (iterate `parsed_edges`/
  `old_parsed_edges` directly with no sort, unlike Phase 4 which already
  sorts via `pathmap_order`) — no test currently asserts relative order,
  but a sort should be added for output-diff readability.
- `check_path_invariants`/`built_in_test` diagnostic message strings join
  `BTreeSet` iteration into human-readable error text — no test asserts
  message order, cosmetic only.
- `apply_projection_steps_to_package`'s `TapeContent::Nodes`/`Compose`
  construction from `BTreeSet` iteration — a *real* production code path
  (existing tests bypass it by hand-constructing tape entries), would
  change from ascending-Bid to hash order for `Identity`/`Filter`/`Compose`
  steps. Needs an explicit `.sort()` at construction if deterministic
  viewer/table output for these steps matters.
- `BeliefGraph.states` is `pub`, `#[derive(Serialize, Deserialize)]`, and
  directly serialized to `beliefbase.json`/msgpack — swapping changes
  on-disk JSON key order (cosmetic; JSON key order isn't semantically
  meaningful and no test asserts on it, but flagged for anyone doing raw
  byte-diffs across builds).
- `find_orphaned_edges` returns `BTreeSet<Bid>` with a doc comment that
  promised "sorted, deduplicated" — nothing downstream actually needs
  sortedness (only dedup); one test (`test_detect_orphaned_edges`,
  `beliefbase/tests.rs`) calls the `BTreeSet`-specific `.pop_first()`, a
  mechanical fix if this return type is migrated. **Doc comment
  corrected** (no longer claims sortedness is guaranteed) — return type
  itself intentionally left as `BTreeSet<Bid>` for now, since
  `find_orphaned_edges` wasn't in this migration's scope (`states`/
  `bid_to_index` only); the `.pop_first()` call site will need the
  mechanical fix noted above whenever this return type is migrated.
- `BidGraph::sink_subgraph`/`source_subgraph` return `BTreeSet<Bid>` but
  have **zero callers anywhere in the repo** — dead public API, free to
  change or flag for removal.

**`states: BTreeMap<Bid, BeliefNode>`** (on both `BeliefBase` and
`BeliefGraph`) is itself category (a) internally (pure keyed lookup
everywhere), but carries the widest public-API surface of anything
audited — see "Scope Decision: `states`" below.

### `src/paths/pathmap.rs`

All findings (a) or (b), **zero (c)** — see the Architecture Notes update
above for the full reversal of the original out-of-scope assumption.
Additional findings beyond the original prompt:

- `PathMap.path_map: BTreeMap<String, usize>` had no live reader outside
  a test helper (`assert_pathmap_indices_consistent`) anywhere in the
  codebase — actual path-string lookups go through a linear scan of
  `self.map` instead (`indexed_get`). **REMOVED** (field, all
  insert/remove/clear maintenance call sites, and the corresponding test
  assertion) as dead weight, independent of the BTree/HashMap question.
- `PathMap.subnets: BTreeSet<Bid>` has a pre-existing multi-match
  tie-break ambiguity (several resolvers pick the first match via
  `.iter().find_map()`; with multiple valid subnets the winner is
  ascending-Bid order, which has no semantic connection to correctness).
  Swapping to `HashSet` doesn't introduce this ambiguity, just makes it
  non-reproducible across runs — the real fix (longest-prefix-match logic)
  is independent of container choice.
- The "ancestor lookup by order prefix" feature the `order_map` doc
  comment describes is actually implemented in
  `codec/builder.rs::try_initialize_stack_from_session_cache` via a linear
  scan of `PathMap.map()` (the sorted `Vec`), not via `order_map` at all —
  confirms `order_map`'s sorted semantics are never consumed by any
  ancestor-walking algorithm.

### `src/query/spec.rs`

11 (a), 6 (b), **zero (c)**. Stronger confirmation than expected: every
test-side `BTreeSet` usage compares via `assert_eq!` or `.contains()`,
both of which are order-agnostic for `BTreeSet` *and* `HashSet` — so
test-refactor cost for this file is genuinely trivial (type-annotation
swaps only, zero logic changes). The one real design smell —
`TapeContent::Compose::result`/`intersection: Vec<Bid>` freezing
`BTreeSet` order into a stored `Vec` — currently has no order-dependent
consumer (`raw_tape.rs` renders per-BID, not by row position), but
deserves a defensive comment at the `.collect()` site when migrated.

Two adjacent, larger-blast-radius items were flagged as explicitly
**out of this file's scope** and should get their own tracking:
`BeliefGraph.states` (already covered above) and
`WeightSet.weights: BTreeMap<WeightKind, Weight>` (properties.rs — only 3
possible keys, no realistic hashing/perf gain, recommend leaving alone).

### `src/shard/export.rs` + `src/shard/search.rs`

All findings (a) or (b), **zero (c)** — but this module has a
**different risk/reward profile** than the `BeliefBase` hot path: these
structures are populated once per compile (export-time bookkeeping, not a
repeatedly-hit hot loop), and several `BTreeMap` fields
(`GlobalShard.bref_index`/`states`, `NetworkShard.states`,
`SearchIndex.docs`/`index`) are `#[derive(Serialize, Deserialize)]` and
written to msgpack/JSON artifacts consumed by the WASM viewer and MCP
server.

Searched explicitly for evidence that byte-reproducible serialization
order is relied upon anywhere (golden-file tests, byte/string-equality
assertions on exported artifacts, documentation claiming reproducibility
is a goal) — **found none**. Every consumer of these serialized maps
(`merge_shards_into_graph`, `query_search_index`, WASM `load_shard`) does
only `.get()`/`.contains_key()`/`.extend()` — order-independent by
construction. The codebase already uses the "sort explicitly at the
output boundary" pattern this issue recommends generally for the *one*
field that does need stable order (`ShardManifest.networks: Vec<...>`,
populated via an explicit `.sort_by_key()`).

**Recommendation for this module specifically: defer to a separate,
lower-priority follow-on issue** (not blocking, not urgent) — different
module, different callers, different risk profile (export-time
bookkeeping vs. hot-path perf) from the core `BeliefBase` migration. One
caution for that follow-on: `shard::wire` structs must keep compiling on
wasm32 (confirmed the migration doesn't touch any `serde_wasm_bindgen::
to_value()` direct-conversion path — these go through `rmp_serde`/
`serde_json` bytes first — but re-verify explicitly rather than assuming
by analogy, per this project's wasm32 track record).

## Scope Decision: `states: BTreeMap<Bid, BeliefNode>` (MIGRATED)

Unlike `bid_to_index` (fully private, narrow call-site shape), `states`
appears in wide public API surface: `BeliefBase::new()`,
`BeliefBase::new_unbalanced()`, `BeliefBase::states()`,
`BeliefGraph.states` (a `pub` field, not an accessor), and is directly
serialized for `beliefbase.json`/msgpack export.

**Originally scoped as a deferred follow-on** (see prior revision of this
section) due to public-API and serialization-order concerns. Revisited
and cleared on further discussion:

- **Shard/export artifacts are ephemeral.** `beliefbase.json`/msgpack are
  regenerated per build, not diffed byte-for-byte or relied upon for
  reproducible key ordering by any tooling found in the audit (see the
  `shard/export.rs`+`shard/search.rs` findings above) — the JSON
  key-order change from this swap is cosmetic and low-risk.

**`states` migrated to `FxHashMap<Bid, BeliefNode>`** on both
`BeliefBase` and `BeliefGraph`, alongside the `bid_to_index` swap.
Internally every call site used only common `Map` operations except two
genuine `BTreeMap`-specific usages, both fixed during the migration:

- `BeliefBase::into_state`'s `.pop_first()` (no direct callers/tests
  found; rewritten to an equivalent `HashMap`-compatible remove-first
  pattern that preserves the original API-node-skip behavior).
- `graph_for_owner`/`add_relations_seeded`/`union_mut_with_trace`'s
  `btree_map::Entry` usage (swapped to `hash_map::Entry`, same
  `Vacant`/`Occupied` API shape).

The audit's category-(b) predictions were also confirmed and fixed: three
tests (`test_union_mut_disjoint_states_commutative`,
`test_union_mut_disjoint_tasks_commutative`,
`test_union_mut_three_way_merge_associative_under_disjoint_ownership` in
`src/beliefbase/graph.rs`) asserted `.keys().collect::<Vec<_>>()` equality
between two independently-built maps — relies on both maps producing the
same iteration order, which `BTreeMap` (sorted) guarantees but `HashMap`
does not. Fixed by comparing as `BTreeSet<Bid>` instead (the actual
invariant under test — set equality — was never about order). Two more
tests (`test_git_metadata_populated_on_network_node`,
`test_metadata_in_exported_json` in `src/codec/compiler.rs`) picked
`network_nodes[0]` after a `.values().filter()` scan to find "the" test
network among several synthetic ones (api/asset/href) — this only ever
worked by incidental Bid-sort luck; fixed by selecting the intended
network by title explicitly, which is more correct regardless of
container type.

## Testing Requirements

- Full existing test suite (`cargo test --all-features`) must continue to
  pass. Any failures caused by iteration-order changes are exactly the
  category (b) cases this issue expects to find and fix — do not treat a
  test failure as a reason to abandon the change without first checking
  whether it's an order-sensitive assertion that should be fixed to sort
  explicitly instead.
- New benchmark comparison: before/after Criterion numbers for
  `document_processing.rs` and `macro_benchmarks.rs`, plus a real corpus
  parse (a `repo` subtree or a full production corpus, depending on
  available time) to confirm the change is worth its complexity.
- If a fast hasher is introduced, add a note/comment at the type alias site
  making clear this is an intentional non-cryptographic choice, so a future
  contributor doesn't "fix" it back to `SipHash` defaults without
  understanding why.

## Success Criteria

- [x] Every BTree usage in `src/beliefbase/*.rs` is classified (a)/(b)/(c)
      per the Goals section, with findings documented (in this issue or a
      linked design note). Expanded to also cover `src/paths/pathmap.rs`,
      `src/query/spec.rs`, and `src/shard/{export,search}.rs` per updated
      scope agreement — see "Audit Findings" above.
- [x] A prototype swap of at least `states` and `bid_to_index` exists and
      passes the full test suite (with any necessary explicit-sort fixes).
      Both migrated to `FxHashMap` (`bid_to_index` in commit `8a48a96`;
      `states` on `BeliefBase`+`BeliefGraph` in a follow-up increment) —
      see "Scope Decision: `states`" above for the full writeup including
      the two `btree_map::Entry`→`hash_map::Entry` fixes and the five
      test fixes (three order-dependent `assert_eq!`s, two `[0]`-index
      selections) the audit predicted and the migration confirmed.
- [x] Benchmark data exists showing the actual performance delta (positive,
      negative, or negligible) — this issue's success is producing that
      data and a recommendation, not necessarily merging the change if the
      data doesn't support it. See `bid_to_index` validation: `graph_queries`
      (lookup-isolated benchmark) improved ~40-43%, reproduced across runs;
      I/O-dominated benchmarks showed no attributable regression.
- [x] `PathMap`/`PathMapMap`'s BTree usage is explicitly confirmed
      out-of-scope (or, if the audit finds it's actually safe to change too,
      that's documented as a discovered expansion of scope — not assumed
      upfront). **Discovered expansion**: audit found `pathmap.rs` is
      actually IN SCOPE (original out-of-scope assumption was refuted) —
      see "Audit Findings" above.

## Risks

- Risk: Hidden category-(b) dependencies are more pervasive than expected,
  turning this into a much larger change than the "swap two fields"
  starting point suggests → **Mitigation**: the audit (Goal 1) happens
  before any code changes; if the blast radius is too large, scope the
  prototype to only the highest-value structures and leave the rest as a
  documented follow-on.
- Risk: The performance win turns out to be negligible (BTree's O(log N)
  vs. HashMap's O(1) may not matter much if N per lookup is small, or if
  the actual bottleneck is elsewhere — e.g. Issue 99/100's findings) →
  **Mitigation**: benchmark early, before investing in a full audit — a
  quick before/after swap of just `states` on a representative benchmark
  can validate or invalidate the premise cheaply.
- Risk: `#[cfg(target_arch = "wasm32")]` dual implementations
  (`RwLock<BTreeMap<...>>` vs. `RefCell<BTreeMap<...>>`) make the change
  more invasive than a single-platform swap would be → **Mitigation**:
  budget time for both code paths; do not assume wasm32 behavior mirrors
  native without testing (wasm test coverage should be checked explicitly).

## Recommendation

**Proceed with a full migration, staged as separate follow-on issues per
module** — the audit found no category (c) blockers anywhere (zero across
all five files/modules examined), confirming the core hypothesis that
BTree's sorted-iteration semantics are not load-bearing for correctness
anywhere in this codebase; every place true ordering matters already
routes through an explicit sort (`WEIGHT_SORT_KEY`, tape order, or a
sorted `Vec` as in `pathmap.rs`).

Staging, in priority order:

1. **DONE**: `BeliefBase.bid_to_index` → `FxHashMap` (commit `8a48a96`).
   Validated performance win, zero test breakage.
2. **DONE**: `states: FxHashMap<Bid, BeliefNode>` (on both `BeliefBase`
   and `BeliefGraph`) — the primary node-lookup table, migrated after
   reassessing the public-API surface and serialization-order concerns (shard
   artifacts are ephemeral, no reproducibility dependency found).
3. **Recommended next**: `src/paths/pathmap.rs` — confirmed in
   scope, all (a)/(b), meaningful traffic (every parsed document touches
   `PathMapMap`/`PathMap` during path generation).
4. **Lower priority, smaller win**: remaining `BTreeMap`/`BTreeSet` locals
   in `base.rs`/`graph.rs` (traversal frontiers, dedup guards) and
   `src/query/spec.rs`'s `Tape`/`QueryPackage` accumulators — these are
   per-call-scoped, so the O(log N) vs O(1) win only matters proportional
   to how large N gets per traversal/query, worth revisiting once (2) and
   (3) land and can be benchmarked together against a large corpus per
   Issue 99.
5. **Deferred, separate issue, not urgent**: `src/shard/export.rs` +
   `src/shard/search.rs` — confirmed safe (zero c, no reproducibility
   dependency found) but different risk profile (export-time bookkeeping,
   not hot-path) and different module/callers; bundling it here would
   dilute focus without adding urgency.

Each follow-on issue should apply the same pattern already proven for
`bid_to_index`: swap the container, add explicit `.sort()` only at the
specific (b)-classified output points identified in the audit above, run
the full test suite, and benchmark before/after on a workload that
isolates the structure from I/O noise (the lesson from this increment's
cheap-validation step: `tests/network_1` is too small at 30 files for
I/O-dominated benchmarks to show a clear signal — either use a
lookup-isolated microbenchmark like `graph_queries`, or benchmark against
a larger corpus per Issue 99).

## References

- `noet-core/src/beliefbase/base.rs` — primary audit target; see the file
  outline for the full list of `BeliefBase` methods, many of which use
  `BTreeSet<Bid>`/`BTreeMap<Bid, _>` internally.
- `noet-core/src/beliefbase/graph.rs` — `BidGraph`, `WEIGHT_SORT_KEY` usage
  in `as_subgraph`/`as_subgraph_seeded` (evidence that sibling ordering is
  already captured via sort key, not map iteration order).
- `noet-core/docs/design/dag_model.md` and `docs/design/query_model.md` —
  background on the Section-edge/`WEIGHT_SORT_KEY` ordering model that
  motivates the "ordering is already captured relationally" argument.
- `noet-core/docs/project/0_open/ISSUE_99_LARGE_CORPUS_PERF_INVESTIGATION.md`
  — shares performance motivation; not a hard dependency.
