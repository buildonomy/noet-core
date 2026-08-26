# Issue 83: BeliefSource Trait Refactoring — QuerySpec as Primitive

**Version**: 0.1
**Priority**: HIGH
**Estimated Effort**: 8 days (RELATIVE COMPARISON ONLY)
**Dependencies**: Requires Issue 79 (QuerySpec infrastructure — complete).
Blocks Issues 80, 81, 82 — building on the Expression-lowering path
means rework when Expression is deleted, and owner-role traversal
(`o-pragmatic-k`) doesn't work due to Trace contamination. Doing 83
first gives 80/81/82 the clean `source.eval(&spec)` API from day one.

## Summary

`QuerySpec` is the query primitive — the canonical representation of what to
query (see `docs/design/query_model.md` §3–§7 for the three-component model
and `docs/design/dag_model.md` §4 for the video-camera analogy that motivates
it). `Expression`, `eval_query`, `balance`, and `submap` are implementation
details of specific evaluation strategies (DB, in-memory). Currently
`BeliefSource` exposes these details as the trait API, forcing every consumer
to construct `Expression` trees and manage traversal/balance logic themselves.

This issue refactors `BeliefSource` so its public API takes `QuerySpec` and
returns an `EvalOutput` determined by the `View` enum, and **deletes
`Expression` entirely**. Each implementation translates `QuerySpec` directly
into its native evaluation strategy and fuses evaluation with output
production: `DbConnection` generates SQL, `BeliefBase` does in-memory graph
traversal, WASM does the initial background parse. No intermediate
`Expression` type exists.

## Goals

- `BeliefSource` primary method: `eval(&QuerySpec) -> EvalOutput`
- `View` is a closed enum, not a trait — each variant is a core
  library type that documents how it flattens a `BeliefSource`
- `eval` is fused: the core library evaluates the query AND produces
  the instrument output in a single pass (no re-fetching)
- `Expression` type **deleted** — each backend translates `QuerySpec`
  directly into its native evaluation
- `submap` expressible as a `QuerySpec` (Subject + section traversal)
- `eval_query`, `eval_unbalanced`, `eval_trace`, `balance`,
  `eval_balanced`, `get_async`, `get_owned_context`, `submap`, `submap_by_bid` — all
  removed
- No intermediate `Expression` type — `QuerySpec` is the sole query
  representation at all layers

## Architecture

### Pre-issue trait (pre-Phase 6)


```
trait BeliefSource {
    eval_unbalanced(&Expression)       // low-level primitive
    eval_trace(&Expression, WeightSet) // balance helper
    eval_query(&Query, bool)           // expression + traversal + balance
    submap(net_bid, path, depth, ...)  // path-based traversal
    submap_by_bid(net_bid, entry, ...) // BID-scoped traversal
    balance(&mut BeliefGraph)          // fill Trace nodes
    eval_balanced(&Expression)         // eval + balance (convenience)
    eval(&Expression)                  // eval + balance + paginate (convenience)
    get_async(&Expression)             // eval + balance + single-result (convenience)
    get_owned_context(Bid)             // MCP-specific context extraction
    export_beliefgraph()               // full graph export
    get_file_mtimes()                  // cache metadata
}
```

### Target trait (post-Phase 7)

```
trait BeliefSource {
    /// Evaluate a query package in place.
    async fn evaluate(&self, package: &mut QueryPackage) -> Result<()>;

    /// Path-based submap traversal (deferred-removal, Phase 8).
    async fn submap(...) -> ...;

    /// BID-scoped submap traversal (deferred-removal, Phase 8).
    async fn submap_by_bid(...) -> ...;

    /// Cache metadata for incremental compilation.
    async fn get_file_mtimes(&self) -> Result<BTreeMap<PathBuf, i64>>;

    /// Full graph export.
    async fn export_beliefgraph(&self) -> Result<BeliefGraph>;
}
```

`get_edges` and `get_node` were removed in Phase 7d, replaced by
`lookup_node` and `lookup_edges` free functions that operate directly
on `BeliefGraph`.

### QueryPackage (lifecycle object)

`QueryPackage` replaces `QueryResult` and `EvalOutput` as the sole
query lifecycle type. It carries the original spec, the effective
(possibly rewritten) spec, the tape, and the output — all in one
object that evaluators populate progressively.

```
struct QueryPackage {
    /// The user's original intent — never mutated after construction.
    original_spec: QuerySpec,
    /// The effective spec — evaluators may append halo/section-roots steps,
    /// rewrite subjects, apply PathMap acceleration, etc.
    spec: QuerySpec,
    /// The tape — populated progressively by evaluators.
    tape: Tape,
    /// The materialized graph — populated by evaluators for graph-context queries.
    graph: Option<BeliefGraph>,
}
```

**Stage is derived, not imperative.** `QueryPackage` has no `stage`
field. Instead, the `stage()` method derives the current lifecycle
phase from internal state:

```
enum PackageStage {
    Constructed,    // spec set, nothing else
    Anchored,       // subject resolved to seed BIDs
    Projecting,     // projection steps partially evaluated
    Projected,      // all projection steps complete (terminal)
}
```

This keeps stage transitions implicit in the data — the evaluator
doesn't manually advance a state machine, it just populates fields
and the derived stage reflects progress. An evaluator receiving a
partially-populated package can inspect `stage()` to decide whether
to resume, skip, or re-evaluate steps.

The design emerged from puzzling through how to pass spec and results
through `BeliefBase` without bolting lifecycle tracking onto stateless
transforms. Making the query a mutable lifecycle object means the
evaluator can inspect state (empty tape? spec missing halo?) and act
accordingly. Spec rewriting becomes data, not special-case code.
Trace coloring falls out of the tape structure — the evaluator-appended
steps are the last N tape entries, and `tape.fold_bids(0..len-N)`
gives the primary set.

**Extension point: `StepOrigin` on `TapeEntry`.** The query lifecycle
has two phases that mutate the package:

1. **Optimizer** (runs first) — rewrites `spec` in place. May replace,
   reorder, or fuse projection steps (e.g. PathMap acceleration
   replaces a `Count(Max)` section traversal with a direct lookup).
   Not append-only — the optimizer may produce a spec structurally
   different from `original_spec`. The diff between them documents
   what was rewritten.

2. **Evaluator** (runs after optimizer) — appends steps to `spec`
   (e.g. halo/section-roots for `View::Graph`) and populates the tape.

Annotating tape entries with origin lets an evaluator receiving a
partially-populated package decide whether to resume, skip, or
re-evaluate steps:

```
enum StepOrigin {
    User,                       // from the original spec
    Evaluator(&'static str),    // "halo", "section-roots"
}
```

Not needed for the initial implementation but a natural extension
once query optimization or multi-evaluator pipelines exist.

### View enum: deleted

**The `View` enum, `EvalOutput` enum, `QueryResult` struct,
`create_instrument` factory, and `produce_output` method were all
deleted in Phase 7a.** View rendering is now consumer-side: the
`Instrument` trait takes `&QueryPackage` and reads tape + graph
directly. `QueryPackage::balanced(spec)` replaces the old
`View::Graph` evaluator branch.

### Trace Kind Separation

`BeliefKind::Trace` marks "this `BeliefGraph` instance doesn't have all of
this node's edges" — a structural completeness property of a particular
graph snapshot, not a query provenance marker. It remains valid for:
- The compiler pipeline (`cache_fetch`, `push`, `push_relation`)
- `BeliefGraph` set operations (`union_mut`, `intersection`)
- Constant-namespace nodes (`External | Trace` = permanently incomplete)

`QuerySpec` projection operates on `BTreeSet<Bid>`, not `BeliefGraph`.
It never reads or writes `BeliefKind::Trace` during projection steps.
When `view == View::Graph`, the materialization phase *does* write
`Trace` onto halo/section-roots nodes — but this happens after all
projection steps complete, so Trace never leaks into set operations.
This cleanly separates two concerns that were conflated in
`evaluate_expression` — and is the root cause of the Trace contamination
bug that breaks owner-role traversal in the Expression-lowering path
(Issue 79 integration test `test_owner_traversal_maps_to` fails because
`evaluate_expression` adds Trace endpoint nodes that leak through
Intersection/Difference boundaries).

### Fused `View::Graph` Pipeline

When `view == View::Graph`, evaluation is a single fused pass. The
pipeline:

1. **Subject** → seed BID set.
2. **Projection steps** → transform the BID set through filters,
   traversals, and compositions. The cumulative union of all step
   outputs = the **primary** BID set.
3. **Halo** — appended to the effective spec by `ensure_graph_context`
   as a regular projection step with `StepInput::Chain` (chains from
   the last user step). Full neighbor glob: `sko-[*]-sko {1}` — for
   each primary BID, discovers all edges where that BID is a Source,
   Sink, **or Owner** (via `WEIGHT_OWNED_BY`), and collects the
   opposite endpoints. This means `View::Graph` automatically includes
   owned-edge endpoints as Trace context, collapsing the old
   `fetch_owned_edges` / `OwnedBeliefContext.owned_edges` into the
   default graph output.
4. **Section section-roots** — also appended by `ensure_graph_context` as
   a projection step with `StepInput::Chain`. Chains from the halo
   step (which already contains the parents via edge completion),
   traversing Section edges upstream to root.
5. **Trace coloring** — a node is `BeliefKind::Trace` iff it appears
   only in steps 3–4 and not in the primary set (step 2). If a node
   was a primary result, it keeps its real `BeliefKind` regardless of
   also appearing in halo/section-roots.

   This is a single set-difference: `trace_only = (halo ∪ section-roots)
   - primary`. O(n) with `BTreeSet` operations. No per-node edge
   checking needed — the pipeline's own BID sets provide complete
   provenance, unlike the old Tape-based algorithm which required
   expensive per-node edge walks for residual classification.

6. **Materialize** — `materialize_graph` is a pure tape-to-graph
   transform. It uses `tape.graph_context_boundary()` to find the
   boundary between primary and Trace tape entries. Collects nodes +
   all edges between result BIDs. Returns
   `EvalOutput::Graph(BeliefGraph)`.

Steps 3–4 replace the current `balance()` method. Because halo and
section-roots are regular projection steps in the spec, there are no
hidden traversals — everything is visible in the effective spec and
recorded in the tape.

**The halo (step 3) is the architectural key to Trace isolation.**
The old `evaluate_expression` added Trace-marked edge endpoints
*during* expression evaluation — inside set operations. This meant
Trace nodes leaked through Intersection/Difference/Union boundaries,
corrupting results (the root cause of the Issue 79 Trace
contamination bug). By deferring endpoint completion to a
post-projection step, the halo guarantees that all set algebra in
step 2 operates on clean, Trace-free BID sets. Trace is purely an
output-side annotation, never an input to query logic. This is what
makes the O(n) set-difference in step 5 correct — the primary set
is authoritative because no Trace nodes were mixed into it.

Step 4 replaces the Section-section-roots walk from `balance()`.

Callers that need `PathMapMap` reconstruction set `view: View::Graph`.
Callers that just need scored BIDs (MCP tools, viewer, `{query}`
directive) use `View::Query` or `View::Table` and skip steps 3–6
entirely.

Because the spec stays as data until `eval_spec` is called, the
`BeliefBase` implementation can execute the entire pipeline
synchronously in one method — no async cycle between "evaluate" and
"materialize."

`eval_query` and `balance` are absorbed into this pipeline.

### TapeFn and ProjectionStep structure

`StepInput` was deleted in Phase 7b, replaced by `TapeFn` which
subsumes input selection and adds terminal/orphan computation:

```
struct ProjectionStep {
    /// Names the tape entry produced by this step (§5.5, §6).
    label: String,
    /// Selects input from the tape.
    input: TapeFn,
    /// What this step does with that input.
    operation: StepOperation,
}

enum TapeFn {
    /// Chain from the previous step (or a named step via StepRef).
    Then(Option<StepRef>),
    /// Fold (set operation) over a range of tape entries.
    Fold { op: SetOp, range: Option<(StepRef, StepRef)> },
    /// Terminal node computation — replaces TraversalSpec::terminal_roles.
    Terminal(Option<(StepRef, StepRef)>),
    /// Orphan node detection.
    Orphan(Option<(StepRef, StepRef)>),
}

enum StepOperation {
    Filter(FilterSpec),
    Traverse(TraversalSpec),
    Compose(ComposeSpec),
}
```

`terminal_roles` was removed from `TraversalSpec` in Phase 7c —
terminal computation is now expressed as `TapeFn::Terminal`, a
first-class tape operation rather than a traversal post-filter.

### Incremental migration path

Phase 1 (✅): Add `eval_spec(&QuerySpec)` with `TraversalDepth`,
`EdgePredicate`, `EvalOutput`. `BeliefBase` overrides with direct
in-memory evaluation. All existing trait methods unchanged.

Phase 2–3 (✅): Migrate MCP tools + codec/compiler consumers from
`Expression` construction to `QuerySpec`. `DirectiveRefiner` returns
`QuerySpec`. MCP tools use `eval_spec_boxed`.

Phase 4 (✅): Fused `View::Graph` pipeline landed (subject →
projection → halo → section-roots → Trace coloring → materialize).
`StepInput`/`StepOperation` restructure, `QueryPackage` lifecycle
object, `BeliefSource::evaluate(&mut QueryPackage)` all complete.
`eval_spec` override removed from `BeliefBase`, `eval_spec_boxed`
removed from `McpBeliefSource`.

Phase 5 (✅): Migrate remaining BeliefBase-side consumers from
`Expression` to `QuerySpec`/`QueryPackage`. `Subject` consolidated
from 8 variants to 5: `Bids` (resolved), `Keys(Vec<NodeKey>)`
(unresolved), `CorpusWide`, `Implicit`, `DocumentNodes`.
Convenience API: `BeliefSource::eval_package()`,
`BeliefBase::eval_as_result()`. `BeliefSource::get_edges(&[Bid])`
added as relation-space primitive. Dead code removed:
`evaluate_query_spec`, `evaluate_query_spec_as_graph`,
`record_tape_sync`, `apply_projection_steps_with_tape`,
`eval_spec` override. 16 call sites migrated. `insert_state`
collision detection replaced `evaluate_expression` with
`self.get(key)` (O(log N) vs O(E)).

Phase 5b (✅): Owner edges memo (`BTreeMap<Bref, BTreeSet<EdgeIndex>>`)
on `BeliefBase`, incrementally maintained. `fetch_owned_edges` deleted.
Halo enriched to `sko-[*]-sko {1}`. `Expression` import removed from
`builder.rs`.

Phase 5c (✅): `OwnedBeliefContext` + `OwnedExtendedRelation` deleted.
`get_owned_context` + `get_owned_context_boxed` deleted from traits.
`BeliefBase::get_context` changed `&mut self` → `&self`.
MCP `get_context`/`get_maps_to`/`get_maps_to_traceability` restructured
to use `QueryPackage` → `BeliefBase` → `BeliefContext<'a>` directly.
WASM `extract_node_context` uses `BeliefContext<'a>` directly.

Phase 6 (next): Delete `Expression` infrastructure. See Step 6
for detailed sub-steps (6a/6b/6c).

Each phase is independently committable and testable.

## Implementation Steps

1. Add `eval(&QuerySpec)` + guided traversal types (1 day)
   - [x] Add `TraversalDepth` struct (`count: DepthCount`, `edge_filter:
         Option<EdgePredicate>`) and `EdgePredicate` struct to `spec.rs` —
         extends `TraversalSpec.depth` from `u8` to `TraversalDepth`
         (see `query_model.md` §5.2)
   - [x] Implement `BeliefBase::evaluate_query_spec(&QuerySpec) -> QueryResult`
         — direct evaluation on BTreeSet<Bid>, no Expression. Handles
         `Count` depth with `EdgePredicate` per-hop filtering.
   - [x] EdgePredicate evaluation: `matches_weight(&Weight)` resolves
         predicate against payload table; `edge_matches_with_filter()`
         wired into `apply_traversal` for Source, Sink, and Owner paths
   - [ ] PathMap acceleration: detect guided section traversals, resolve
         against `self.paths()` in O(1) instead of graph walk. **Deferred
         to Phase 8** — performance optimization, not correctness.
   - [x] `BeliefSource::eval_spec` (will become `eval` when Expression is
         deleted) default delegates to `QuerySpec::evaluate`;
         `BeliefBase` overrides with direct `evaluate_query_spec`
   - [x] All existing trait methods unchanged — additive only
   - [x] Test: all 13 integration tests pass (including
         `test_owner_traversal_maps_to`, `test_table_instrument_maps_to`,
         and `test_eval_spec_matches_evaluate_query_spec`)

2. Migrate MCP tools (1 day)
   - [x] Fix inline imports in `tools.rs` — 14 inline `use` statements
         moved to module top; fully-qualified `std::collections::*` paths
         replaced with bare names
   - [x] `get_context` — no migration needed: uses `get_owned_context_boxed`,
         no Expression construction
   - [x] `get_submap` — still uses `submap_by_bid_boxed` (kept on trait).
         No Expression construction; no migration needed for 6c.
   - [x] `get_traceability` — same as get_submap. No Expression.
   - [x] `get_maps_to` — migrated to `View::Graph` + `get_context`
         in Phase 5c. No longer uses `export_beliefgraph_boxed`.
   - [x] `get_maps_to_traceability` — same pattern, migrated in 5c.
   - [x] `query` tool — migrated to `QuerySpec` JSON in Phase 6a.
   - [ ] Remove `McpBeliefSource` wrapper trait if possible (it exists
         to box futures — may be unnecessary with `eval`). **Deferred
         to Phase 8** — still used by 5 MCP tools.
   - [x] Delete dead Focus infrastructure: done in previous session
   - [ ] Change `View::render` signature: `&BeliefGraph` →
         `&dyn BeliefSource` — views query the source directly.
         **Deferred to Phase 7** — part of tape granularity refactor.
   - [ ] Update `TableInstrument` to query `BeliefSource` for node data
         instead of reading from a pre-materialized `BeliefGraph`.
         **Deferred to Phase 7** — part of view layer refactor.

3. Migrate compiler/codec (2 days)
   - [x] `BeliefBase::evaluate_query_spec_as_graph` — materializes a
         `BeliefGraph` from QueryResult BIDs (nodes + internal edges).
         Stepping stone toward `View::Graph` in fused eval.
   - [x] Directive refiners in `myst.rs` — all three migrated to QuerySpec.
         `TraversalSpec` with `input_roles`/`output_roles` covers all
         `RelationPred` patterns: `SinkIn` → `input: Sink, output: Source`;
         `OwnedBy` → `input: Owner, output: {Source, Sink}`;
         `NetPathIn` → Section traversal from network root (`DepthCount::Max`).
         `DirectiveRefiner` type alias returns `QuerySpec` instead of `Expression`.
   - [x] Builder code in `builder.rs` — all Expression construction
         sites migrated. `fetch_owned_edges` replaced by owner memo
         (Phase 5b). `cache_fetch`/`initialize_stack`/`push_relation`
         use `QueryPackage` + `evaluate` (Phase 6b). Expression import
         removed.
   - [x] Compiler code in `compiler.rs` — `parse_epoch`,
         `generate_html_for_path`, `sync_asset_snapshot` all migrated
         to `QueryPackage` + `Subject::Bids`/`Subject::DocumentNodes`
         (Phases 5c/6a/6b). No Expression construction remains.
   - [x] Compiler `OwnedBy` sites in `compiler.rs` — Step 0b pre-fetch
         and maps_to sentinel path migrated from `RelationPred::OwnedBy`
         to `QuerySpec` with Owner traversal. Inline imports removed.

4. QueryPackage + fused View::Graph (1.5 days)
   - [x] Spec-correct traversal: `apply_traversal` returns discovered-only
         (not seed ∪ discovered), per query_model.md §5.2.
   - [x] Fused View::Graph pipeline: halo + section-roots as `TraversalSpec`
         applications, Trace coloring via set-difference.
   - [x] `Tape::fold_bids(range)` and `Tape::cumulative_bids()` for
         cumulative BID tracking across projection steps.
   - [x] `apply_projection_steps_with_tape` builds Tape during evaluation.
   - [x] Define `QueryPackage` type — replaces `QueryResult` and `EvalOutput`
         as the single query lifecycle object. Stage is derived from
         internal state via `stage()` method (`PackageStage` enum), not
         stored as a field.
   - [x] `BeliefSource::evaluate(&mut QueryPackage)` — evaluator inspects
         package state, rewrites spec for View::Graph (append halo/section-roots),
         populates tape, produces output.
   - [x] `eval_spec` override removed from `BeliefBase`, `eval_spec_boxed`
         removed from `McpBeliefSource`.
   - [x] `StepInput`/`StepOperation` added — `ProjectionStep` restructured
         into `{ input: StepInput, operation: StepOperation }`.
   - [x] Migrate remaining builder/compiler sites from `eval_query`
         + `to_query()` to `QueryPackage::new(spec)` + `evaluate()`.
         Done across Phases 5c, 6a, 6b.
   - [x] Move `eval_unbalanced`, `eval_trace` from trait to impl blocks
         on `BeliefBase` and `DbConnection` (no longer public API).
         Done in Phase 6b — both deleted entirely from trait and impls.
   - [x] Remove convenience methods from trait: `eval_balanced`, `get_async`,
         `get_owned_context`, `eval_query`, `balance`. All removed
         across Phases 5c and 6a/6b.

5. Owner-enriched halo + owner memo (1 day)
   - [x] Change `TraversalSpec::halo()` from `sk-[*]-sk {1}` to
         `sko-[*]-sko {1}` — add `Role::Owner` to both `input_roles`
         and `output_roles`. This makes `View::Graph` automatically
         discover owned-edge endpoints as Trace context.
   - [x] Add owner memo index to `BeliefBase`:
         `BTreeMap<Bref, BTreeSet<EdgeIndex>>` maintained incrementally
         by `update_relation`, `replace_bid`, `remove_nodes`, `trim`.
         Built from scratch in `new_unbalanced`. Cloned on `Clone`.
         Enables O(1) owned-edge queries via `graph_for_owner(&Bref)`.
   - [x] Delete `fetch_owned_edges` in builder.rs — replaced by
         `session_bb.graph_for_owner(&owner_bref)` which uses the
         `owner_edges` memo. `Expression` import removed from
         builder.rs (now unused).
   - [x] Test: all 722 tests pass, `test_owner_edges_memo_consistency`
         validates memo is bidirectionally consistent with graph.

5c. Collapse `get_owned_context` → `QueryPackage` + retire `OwnedBeliefContext` (1.5 days)

   **Design insight**: `OwnedBeliefContext` is a redundant intermediate
   representation. The actual data comes from a `BeliefGraph` (which
   `View::Graph` already produces via the `sko` halo). Both MCP and WASM
   consumers immediately destructure `OwnedBeliefContext` into their own
   output types and discard it.

   The correct model: evaluate `QuerySpec::graph(Subject::Bids(bids), [])`
   to get a `BeliefGraph` → `BeliefBase`, then construct `BeliefContext<'a>`
   views at the use site. The `BeliefBase` is the owned object that crosses
   async boundaries — not a pre-materialized context struct.

   For bulk operations (`get_context_bulk`, `get_maps_to`), a single
   `View::Graph` evaluation over all requested BIDs produces one graph;
   per-BID `BeliefContext` views are cheap against that shared graph.

   - [x] Delete `get_owned_context` from `BeliefSource` trait (~55 lines).
         No unique logic — body was `QuerySpec::graph` → `BeliefBase::from`
         → `get_context` → `OwnedBeliefContext::from_ref`.
   - [x] MCP `get_context`: inlined `QueryPackage` → `evaluate_boxed`
         → `BeliefBase::from(graph)` → `BeliefContext` at use site.
         No longer uses `OwnedBeliefContext`.
   - [x] Delete `McpBeliefSource::get_owned_context_boxed` from trait
         + both impls (BeliefBase, DbConnection).
   - [x] `BeliefBase::get_context` changed from `&mut self` to `&self`.
         The `&mut` was a legacy artifact; body only does `&self` ops.
   - [x] WASM `extract_node_context`: restructured to use
         `BeliefContext<'a>` directly. Single shared borrow replaces
         two-phase mutable/shared dance. `resolve_related_path`
         refactored to take individual fields.
   - [x] `OwnedBeliefContext` + `OwnedExtendedRelation` deleted.
         `BeliefContext<'a>` is the sole context type. ~90 lines
         removed from `context.rs`. Re-export cleaned in `mod.rs`.
   - [x] WASM `get_context_bulk`: already efficient — operates on the
         pre-loaded `BeliefBase` directly, no per-BID graph
         materialization. No changes needed.
   - [x] MCP `get_maps_to`: replaced `export_beliefgraph_boxed()` +
         O(E) scan with single `View::Graph` evaluation for owner
         BIDs → per-owner `get_context` → `all_owned_edges()`.
         `WEIGHT_OWNED_BY` import removed from `tools.rs`.
   - [x] MCP `get_maps_to_traceability`: same pattern. Replaced
         full-graph export with `View::Graph` for submap BIDs.
   - [x] `metadata.js` / `traceability.js`: no changes needed.
         `metadata.js` never referenced `owned_edges` directly.
         `traceability.js` reads `owned_edges` from WASM `NodeContext`,
         which is populated from `BeliefContext::all_owned_edges()`
         — already uses the halo-populated graph, not secondary queries.
   - [x] Test: all 722 tests pass, MCP tools use `get_context` path.

6. Delete Expression infrastructure (2–3 days)

   **Investigation summary** (from Phase 5c session):
   `BeliefBase` already has a parallel direct evaluation path
   (`evaluate_query`) that bypasses Expression entirely for in-memory
   work. `DbConnection` still goes through `eval_spec` → `eval_query`
   → `eval_unbalanced` → `AsSql`. The `spec.rs` bridge layer
   (`Subject::to_expression`, `QuerySpec::to_query`) intentionally
   lowers QuerySpec → Expression for the DB path.

   **Remaining Expression callers** (load-bearing after 6a):
   - `db.rs`: `eval_unbalanced` + `eval_trace` overrides → `AsSql`
   - `watch.rs`: `connection.eval_query(&pq.query, false)`
   - `graph.rs`: `build_balance_expr`, `build_upstream/downstream_expr`,
     `to_event_stream` → construct `Expression` for balance loop
   - `accumulator.rs`: forwarding impls for `eval_query`,
     `eval_unbalanced`, `eval_trace`
   - `base.rs`: `evaluate_expression`, `evaluate_expression_as_trace`,
     `filter_states`, `eval_query` cache override, `NoCacheRef`

   6a. Migrate remaining callers to `QueryPackage` / direct lookup (1.5 days)
   - [x] Remove dead code: `eval_balanced`, `eval`, `eval_package`
         from trait; `filter_states_mut`, `EvalQueryTiming`,
         `eval_query_timed` from `BeliefBase`; `eval_query_boxed`
         from `McpBeliefSource`; `find_orphaned_nodes` from
         `BeliefGraph`; `neighborhood_total_us` fields from
         `GraphBuilder`.
   - [x] Add `get_node` default to `BeliefSource` trait (wraps
         existing Expression path). Add `get_node_boxed` to
         `McpBeliefSource`.
   - [x] `builder.rs` + `compiler.rs` `get_async()` (5 sites):
         replaced with `get_node()`. `get_async` deleted from trait.
   - [x] `wasm.rs` `get_networks()` / `get_documents()`: replaced
         `evaluate_expression(StatePred::Kind(...))` with direct
         `states().values().filter(...)`. `EnumSet` import removed.
   - [x] `wasm.rs` `query()`: migrated from `Expression` JSON to
         `QuerySpec` JSON via `QueryPackage` + `evaluate_query`.
         `Expression`/`StatePred` imports removed from `wasm.rs`.
   - [x] `mcp/tools.rs` `query()`: migrated from `Expression` JSON
         to `QuerySpec` JSON via `QueryPackage` + `evaluate_boxed`.
         `eval_unbalanced_boxed` deleted from `McpBeliefSource`
         trait + both impls. `Expression` import removed from
         `tools.rs` and `state.rs`.
   - [x] `watch.rs` `get_states()`: deleted along with
         `PaginationCache`, `PaginatedQuery`, `ResultsPage`,
         `BeliefGraph::paginate`, `DEFAULT_LIMIT`/`DEFAULT_OFFSET`,
         `Op::GetStates`, `OpResult::Page`. No existing use case
         requires pagination; re-add from blueprint if needed.
   - [x] `graph.rs` `to_event_stream` balance loop: replaced
         `build_balance_expr` (Expression) with direct
         `find_externals(Section, Outgoing, true)` call. Returns
         `BTreeSet<Bid>` directly, no Expression construction or
         destructuring. Convergence check uses `BTreeSet` equality
         instead of `Expression` equality.

   6b. `DbConnection::evaluate` override (1 day)
   - [x] Implement `DbConnection::evaluate(&mut QueryPackage)` with
         SQL-native pipeline (no `eval_unbalanced`/`balance`):
         - `resolve_subject(Subject)` → `Vec<Bid>` via SQL
         - `ensure_graph_context` appends halo + section-roots for
           `View::Graph` (same as `BeliefBase`)
         - `apply_traversal_sql` — per-hop SQL against `relations`
           table, filtered by `kind_filter` columns. Handles
           Source/Sink/Owner input roles and output role collection.
         - `apply_filter_sql` — SQL state fetch + in-memory
           `NodeFilter` predicate evaluation via temporary
           `BeliefBase`.
         - `apply_steps_sql` — recursive sub-pipeline for
           `Composition` branches.
         - Bulk-fetch all accumulated BIDs (seed + tape), build
           `BeliefBase`, delegate to `produce_output` /
           `materialize_graph` for Trace coloring.
         - `apply_filter` and `ensure_graph_context` promoted to
           `pub(crate)` on `BeliefBase`.
   - [x] Migrate `BeliefAccumulator` / `QueryHandle` to override
         `evaluate` with cache. Cache keyed on serialized `QuerySpec`
         JSON (avoids `Hash`/`Eq` on `QuerySpec`). Cache stores
         full `QueryPackage` (spec + tape + output) — cache hits
         restore complete state with zero re-evaluation.
   - [x] Remove `eval_spec`, `eval_query`, `balance` from trait.
         `evaluate` default now returns error (all backends override).
         `NoCacheRef` shim deleted. `BeliefBase::eval_query` override
         deleted. `QuerySpec::evaluate`, `record_tape`, post-filter
         helpers deleted from `spec.rs`.
   - [x] Add `evaluate` override to `impl BeliefSource for &BeliefBase`
         (was using trait default which now errors).
   - [x] Remove `eval_unbalanced`, `eval_trace` from `BeliefSource`
         trait. `get_node` and `get_edges` defaults rewritten to
         use `evaluate` via `QueryPackage`. `export_beliefgraph`
         default returns error (all backends override). Accumulator
         forwarding removed. `DbConnection::eval_unbalanced` and
         `eval_trace` (~235 lines) deleted from trait impl.
   - [ ] Recursive CTE for unbounded traversals: detect
         `DepthCount::Max` in `apply_traversal_sql` and emit
         `WITH RECURSIVE` instead of per-hop queries. **Deferred**:
         performance optimization, not blocking Expression deletion.
         Separate issue or BACKLOG.
   - [x] Add `terminal_roles: EnumSet<Role>` to `TraversalSpec`
         (default empty). Implement post-fold filter in
         `BeliefBase::apply_traversal` (in-memory graph scan) and
         `DbConnection::apply_traversal_sql` (SQL `NOT IN` subquery).
         Added `TraversalSpec::roots()` and `TraversalSpec::leaves()`
         helpers. Tests: `test_terminal_roles_roots`,
         `test_terminal_roles_leaves`. All 17 construction sites
         updated with `terminal_roles: EnumSet::empty()`.
   - [x] Move `ensure_graph_context` from `BeliefBase` to
         `QueryPackage` as `pub(crate)` associated function in
         `spec.rs`. Callers in `evaluate_query` and
         `DbConnection::evaluate` updated. No logic change.
   - [x] Add traversal fixture tests in `spec.rs` `#[cfg(test)]`:
         7-node section tree fixture (`api ← net ← doc_a ← sec1/sec2`,
         `net ← doc_b ← sec3`). Tests: `test_traversal_halo`,
         `test_traversal_balance_map`, `test_traversal_roots`,
         `test_traversal_leaves`. Shared helpers: `traversal_fixture()`,
         `eval_traversal()`.
   - [x] `impl BeliefSource for BeliefGraph` (or `&BeliefGraph`):
         wraps `BeliefBase::from(graph)` + `evaluate`. Gives
         `BeliefGraph` the full query model API. `get_edges` and
         `get_node(Bid)` operate directly on graph fields (no
         conversion). `evaluate`/`submap` convert to temporary
         `BeliefBase`. Tests: `test_belief_source_evaluate_on_graph`,
         `test_belief_source_get_node_bid_lookup`,
         `test_belief_source_get_edges_on_graph`.
   - [x] Migrate `BeliefGraph` off `RelationPred`/`Expression`:
         - `BidGraph::filter(&RelationPred)` deleted in Phase 6c
           (only callers were evaluate_expression, also deleted).
         - [x] `find_externals` → refactored to accept `EnumSet<WeightKind>`
           instead of `Option<WeightSet>`, eliminating `RelationPred`
           from graph.rs. Uses direct edge iteration + `GraphMap` for
           the filtered subgraph. `to_event_stream` caller updated.
         - [x] `shard/export.rs` → replaced `RelationPred::NodeIn`
           filter with direct edge iteration. Inline `petgraph::visit`
           imports moved to module top. `RelationPred` references
           removed from comments.
   - [ ] Remove `submap`/`submap_by_bid` from trait. **Deferred to
         Phase 8** — needs `TraversalSpec::submap(depth)` helper +
         View-layer path/order resolution. Many callers across
         compiler, MCP, DB.
   - [ ] Add `capabilities()` method to `BeliefSource` trait.
         **Deferred to Phase 8** — design question, separate issue.
   - [x] Migrate `spec.rs` bridge: remove `Subject::to_expression`,
         `QuerySpec::to_query`. **Folded into 6c** — these exist
         solely to lower QuerySpec → Expression.

   6c. Delete Expression module (0.5 day)
   - [x] Move `BeliefSource` trait definition to `query/mod.rs`.
         `BALANCE_CUTOFF` and `MAX_TRAVERSAL` moved alongside.
         `WrappedRegex` moved to `query/spec.rs`.
   - [x] Delete `Expression`, `StatePred`, `SetOp`, `Query`,
         `RelationPred` types.
   - [x] Delete `AsSql` trait + impls. `db.rs` refactored to use
         `get_states_by_bids(&[Bid])` with inlined SQL helpers
         (`push_id_expr`, `push_string_expr` moved as private fns).
   - [x] Delete `evaluate_expression`, `evaluate_expression_as_trace`,
         `filter_states` from `BeliefBase`.
   - [x] ~~Delete `build_balance_expr`, `build_upstream_expr`,
         `build_downstream_expr` from `graph.rs`.~~ Already done
         (Phase 6b this session). `find_externals` refactored to
         use `EnumSet<WeightKind>` — kept, not deleted.
   - [x] Delete remaining `spec.rs` lowering functions:
         `Subject::to_expression`, `NodeFilter::to_expression`,
         `pred_to_expression`, `CompositionOp::to_set_op`,
         `QuerySpec::to_expression`, `QuerySpec::to_query`,
         `build_owned_by_union`, `filter_by_output_roles`,
         `parse_belief_kind`, and 16 associated test functions.
   - [x] Delete `src/tests/expression.rs` test file. Surviving
         `subsection_chain_balancing` test moved to
         `src/tests/query.rs`.
   - [x] Delete `src/query/expression.rs` entirely.
   - [x] Rename `src/query/instrument/` → `src/query/view/`.
         Type renames (`Instrument` → `ViewRenderer`, etc.)
         deferred — directory + module path renamed only.
   - [x] Delete dead `query_cache` infrastructure from `BeliefBase`:
         `QueryCacheKey` type alias, `query_cache` field,
         `with_query_cache()`, `invalidate_query_cache()`,
         `invalidate_query_cache_for_bids()`, all call sites.
   - [x] Delete `BidGraph::filter(&RelationPred)` and
         `BidRefGraph::filter(&RelationPred)` from `graph.rs`.
   - [x] Remove `.with_query_cache()` call from `builder.rs`.
   - [x] Remove debug `to_expression()`/`evaluate_expression()`
         block from `tests/codec_test/query_tests.rs`.

7. Tape granularity refactor (2–3 days)

   Restructure the tape and `QueryPackage` so the tape is the sole
   interface between projection and consumers. See `query_model.md`
   §5–§7 for the full spec.

   7a. View extraction + `EvalOutput` removal (2 days)

   **Design change (query_model.md §7)**: `View` is now a consumer-side
   trait, not an enum field on `QuerySpec`. The evaluator never branches
   on display intent. `QueryPackage::balanced(spec)` replaces the old
   `View::Graph` evaluator branch. `EvalOutput`, `QueryResult`,
   `produce_output`, and `materialize_graph` are deleted.

   - [x] Add `QueryPackage::balanced(spec)` constructor: calls
         `append_graph_context` internally (private), returns a
         package with halo + section-root steps appended.
   - [x] Remove `view: View` field from `QuerySpec`. Delete
         `QuerySpec::graph()` convenience constructor.
   - [x] Migrate ~30 `View::Graph` construction sites to
         `QueryPackage::balanced(spec)` (builder.rs, compiler.rs,
         myst.rs, nodekey.rs, mcp/tools.rs, mcp/state.rs, wasm.rs).
   - [x] Migrate ~23 `View::Query`/`View::default()` construction
         sites to bare `QuerySpec { subject, projection }` +
         `QueryPackage::new(spec)`.
   - [x] Remove `View::Graph` branch from `evaluate_query` (base.rs)
         and `DbConnection::evaluate` (db.rs). Evaluator guards
         materialization with `graph().is_none()` instead.
   - [x] Delete `produce_output` from `BeliefBase`. `materialize_graph`
         promoted to `pub(crate)`, called directly by evaluators.
   - [x] Delete `EvalOutput` enum. `QueryPackage` has
         `graph: Option<BeliefGraph>` with `set_graph()`, `graph()`,
         `into_graph()`. `set_output`/`into_output` deleted.
   - [x] Delete `QueryResult` struct. Consumers read tape entries
         directly. `eval_as_result` and `into_result` deleted.
   - [x] Delete `View` enum + `View::default()`. `SortSpec` remains
         in `spec.rs` (used by `TableInstrument` directly).
   - [x] Update `Instrument` trait signature from
         `render(&QueryResult, &BeliefGraph)` to
         `render(&QueryPackage)`. Delete `create_instrument` factory.
         Trait not yet renamed to `View` (deferred to avoid churn).
   - [x] Update `BeliefAccumulator` cache: guards on
         `graph().is_some()` instead of `has_output()`.
   - [x] Delete `PackageStage::Evaluated` — `Projected` is now the
         terminal stage. `stage()` no longer checks `graph.is_some()`.
   - [x] Fix `Instrument::render` entry extraction to use
         `tape.graph_context_boundary()` for last user step.

   7b. `TapeEntry` restructure + per-hop recording
   - [x] Add `label: String` to `ProjectionStep` (always present,
         defaults to step index `"0"`, `"1"`, …).
   - [x] Add step label `label: String` to `TapeEntry`. Multi-hop
         traversals produce multiple entries sharing the same label;
         hop index is derived from position within the label group
         (no explicit `hop` field needed).
   - [x] Replace `StepInput` with `TapeFn` enum — four variants:
         - `Then(Option<StepRef>)` — chain from prior step
           (`THEN` in surface grammar; `None` = implicit chain,
           replaces old `StepInput::Chain`)
         - `Fold { op: SetOp, range }` — accumulate over a range
           (`FOLD` in surface grammar; `SetOp` keywords:
           `UNION`, `INTERSECT`, `LDIFF`, `RDIFF`, `SYMDIFF`;
           `Fold { op: Union, range: None }` replaces old
           `StepInput::Cumulative`)
         - `Terminal(range)` — outputs \ inputs across range
           (`TERMINAL` in surface grammar)
         - `Orphan(range)` — inputs \ outputs across range
           (`ORPHAN` in surface grammar)
   - [x] Change `TapeEntry.result` from `BTreeMap<Bid, Score>` to
         `TapeContent` enum:
         - `Edges { edges, output_bids }` — per-hop edge indices
           into package graph + self-contained output BIDs,
           ordered by `WEIGHT_SORT_KEY`
         - `Nodes(Vec<Bid>)` — filter survivors
         - `Compose { op, left, right, result }` — set-op branches
           with `CompositionOp` stored directly in the variant
   - [x] Add optional `payload: Option<Vec<SortPayload>>` on
         `TapeEntry` for TF-IDF scores (most queries: `None`).
   - [x] Change `apply_traversal` to record per-hop tape entries
         (one per hop, not one per step) via new
         `apply_traversal_to_tape`. Empty traversals push one
         empty Edges entry for stage detection. Terminal-filtered
         traversals push a final Nodes summary entry.
   - [x] Delete `TapeEntry.step: ProjectionStep` field (replaced
         by `label`). Delete `TapeEntry.branches` (absorbed into
         `TapeContent::Compose`). Delete `StepInput` enum.
   - [x] Add well-known labels `GRAPH_CONTEXT_HALO_LABEL` and
         `GRAPH_CONTEXT_BALANCE_LABEL` for graph context steps.
         `graph_context_boundary()` matches on label.
   - [x] Update `stage()` to compare last tape entry label to last
         projection step's effective label (handles multi-hop entries).

   7c. Terminal roles → tape derivation
   - [x] Remove `terminal_roles` field from `TraversalSpec` and all
         17 construction sites. Delete post-filter logic from
         `apply_traversal`, `apply_traversal_to_tape`, and
         `apply_traversal_sql` (~180 lines removed).
   - [x] Implement `Tape::eval_input(&TapeFn, seed, chain, prev_label)`
         — general TapeFn evaluator dispatching Then, Fold, Terminal,
         Orphan. Terminal/Orphan compute `union(outputs) \ union(inputs)`
         / `union(inputs) \ union(outputs)` across resolved entry ranges.
   - [x] Add `Tape::entries_for(label)`, `resolve_range`, `resolve_step_ref`
         helpers for TapeFn range resolution.
   - [x] Convert `roots()` and `leaves()` from `TraversalSpec` constructors
         to standalone `pub fn` returning `Vec<ProjectionStep>` — a
         traversal step followed by `TapeFn::Terminal(None)` + pass-all
         filter. Tests updated to use multi-step projection.
   - [x] Use `Tape::eval_input` in projection loop
         (`apply_projection_steps_to_package`) replacing inline
         `match &step.input` with delegated dispatch.

   7d. Trait surface reduction
   - [x] Remove `get_edges` and `get_node` from `BeliefSource`
         trait. Remove `get_edges_boxed` and `get_node_boxed`
         from `McpBeliefSource` trait + both impls.
   - [x] Add standalone `lookup_node` / `lookup_edges` free
         functions in `query/mod.rs` and object-safe
         `lookup_node_boxed` / `lookup_edges_boxed` in
         `mcp/state.rs`. Same logic, not trait methods.
   - [x] Delete optimized overrides from `BeliefBase`,
         `BeliefGraph`, `&BeliefGraph` (~200 lines removed).
   - [x] Migrate 5 callers in builder.rs, compiler.rs, tools.rs.

   7e. Tape API
   - [x] `eval(&TapeFn, seed, prev_label)` — general-purpose
         post-eval evaluator; delegates to `eval_input` with
         chain derived from last entry.
   - [x] `eval_input(&TapeFn, seed, chain, prev_label)` —
         evaluation-time TapeFn resolver (added in 7c).
   - [x] `entries_for(label)` — all entries sharing a label name
         (added in 7c).
   - [x] `last_entry_for(label)` — last non-empty entry for label.
   - [x] `get(idx)` — single entry by index.
   - [x] `output_bids(idx)` — output BIDs for a single entry.
         Graph/spec params removed — `TapeContent::Edges` carries
         `output_bids` directly (self-contained tape).
   - [x] `input_bids(idx, seed)` — input BIDs for a single entry,
         derived from previous entry's output (or seed if first).
   Cumulative and terminal accessors are expressed as `TapeFn`
   variants: `Fold { op: Union, range }` and `Terminal(range)`.

   7f. Consumer updates
   - [x] Simplify `to_event_stream`: replaced ~170-line manual
         halo + balance loop + DFS traversal with
         `QueryPackage::balanced` + tape iteration (~90 lines
         total). Tape provides topological + sibling ordering;
         lhs-wins dedup applied during edge iteration.
   - [x] Synchronize `DbConnection::evaluate` with tape changes:
         - `apply_traversal_sql` now builds edges incrementally
           into the package graph (`SELECT *` → `BeliefRelation`
           → `add_edge` → `TapeContent::Edges`)
         - Uses `Tape::eval_input` for all `TapeFn` dispatch
           (including `Terminal`/`Orphan`)
         - Step 3 merges states into existing graph (edges
           already present from traversal)
   - [x] `BeliefGraph` BeliefSource impl delegates to
         `BeliefBase::evaluate_query` — already synchronized.
   - [x] `find_externals` is now dead code (last caller was
         old `to_event_stream`); retained with `#[allow(dead_code)]`.

8. Deferred cleanup (1–2 days)
   - [x] Instrument → View type renames: `Instrument` →
         `ViewRenderer`, `InstrumentOutput` → `ViewOutput`,
         `TableInstrument` → `TableView`. Directory already
         renamed (`query/view/`); type names updated in Phase 8.
   - [x] Make `BeliefSource` trait object-safe: all 5 methods
         converted from `impl Future` return types to
         `BoxFuture<'a, T>` (boxed futures). Trait bound changed
         from `Sync` to `Send + Sync`. 9 impl blocks updated.
         `lookup_node`/`lookup_edges` free functions now `?Sized`
         (work with `&dyn BeliefSource`). One heap allocation per
         call — negligible vs query/DB work.
   - [x] Remove `McpBeliefSource` wrapper trait. `BeliefSource`
         is now object-safe (`dyn BeliefSource` works directly).
         Deleted: `McpBeliefSource` trait + 2 impls (BeliefBase,
         DbConnection) + `lookup_node_boxed` + `lookup_edges_boxed`
         (~250 lines). `McpState::source_ref()` returns
         `&dyn BeliefSource`. All MCP tools call trait methods
         directly (no `_boxed` suffixes).
   - [x] `BeliefBase::export_beliefgraph` changed from
         `self.clone().consume()` (spin-waits on relations lock)
         to `QueryPackage::new` with all BIDs (no halo/balance).
         Avoids blocking tokio `current_thread` executor. Produces
         raw graph matching DB export semantics.
   - [ ] Recursive CTE for `DepthCount::Max` in
         `apply_traversal_sql` — emit `WITH RECURSIVE` instead of
         per-hop queries. **Deferred to BACKLOG**: `max_hops()`
         clamps `Max` to `MAX_TRAVERSAL` (10), so the per-hop
         loop is bounded. Real-world section depth is typically
         3–5. Three input roles (Source, Sink, Owner) would each
         need a separate CTE branch. Complexity vs. payoff is
         unfavorable.
   - [ ] Remove `submap`/`submap_by_bid` from `BeliefSource` trait.
         Not removed in Phase 8 — kept on trait because compiler
         functions (`finalize_html`, `generate_sitemap`,
         `parse_epoch`) use `B: BeliefSource + Clone` generically
         with both `BeliefBase` and `DbConnection`. Removing
         requires either a second trait or concrete type bounds.
         **Deferred to future issue** — the methods are correct
         and the trait is now object-safe regardless.
   - [ ] Add `capabilities()` method to `BeliefSource` trait —
         returns backend expressivity limits. **Deferred to Phase 9**
         as a documentation/diagnostics review rather than a
         runtime API.
   - [ ] PathMap acceleration: detect guided section traversals,
         resolve against `self.paths()` in O(1) instead of graph
         walk. Performance optimization, not correctness.
         **Deferred to BACKLOG**.

9. Documentation revision + downstream audit + performance validation (1 day)
   - [x] Audit/upgrade the downstream C++ codec consumer crate for API
         compatibility. Only issue: one stale `QuerySpec::graph()`
         call in `noet-core/src/mcp/tools.rs` (fixed — was missed
         in Phase 7a). The downstream crate compiles and tests pass
         against the Phase 8 working tree.
   - [x] `docs/design/beliefbase_architecture.md` — §3.5
         (`BeliefGraph` vs `BeliefBase` updated for `QueryPackage`),
         §3.8 (Query Cost Model: `View::Graph` → balanced
         `QueryPackage`). 6 edits total.
   - [x] `docs/design/query_model.md` — §9.5.9 (MCP surface
         binding updated to `QuerySpec` JSON), §10 (Implementation
         Notes: `QuerySpec` as sole primitive, no Expression
         lowering), §10.1 (trait summary table: 5 methods with
         BoxFuture), §10.2 (Deleted Constructs reorganized into
         categorized groups). 4 sections updated.
   - [x] `docs/design/dag_model.md` — §4 and §6 verified:
         no stale references, uses abstract language consistent
         with `QuerySpec` as primitive. No changes needed.
   - [x] `docs/architecture.md` — verified: high-level overview
         already consistent with current `BeliefSource` surface.
         No references to deleted APIs. No changes needed.
   - [x] Remove stale references to deleted APIs from design
         docs and code comments:
         - `mapping_node_architecture.md`: 4 edits (RelationPred
           → TraversalSpec, `src/query.rs` → `src/query/spec.rs`)
         - `myst_directive_architecture.md`: 4 edits (RelationIn/
           SinkIn/SourceIn → Section/Pragmatic traversals)
         - `BACKLOG.md`: 3 edits (AsSql → evaluate pipeline)
         - Code comments: 37 stale references fixed across 12
           `.rs` files (builder.rs 16, compiler.rs 6, tests.rs 4,
           accumulator.rs 3, mcp 3, others 5).
   - [x] Compile a large systems-engineering corpus (~32k nodes, 58 networks) end-to-end.
         Baseline (pre-Issue 83): 2m 28s, 5,331 warnings.
         Post-refactor: 3m 17s, 5,274 warnings (−57).
         Wall-time delta caused by debug logging regression
         (268k extra lines from `subnet_registration` and
         `db::apply_traversal_sql` at debug level). Fixed:
         demoted to trace. Warning improvements: "No path
         found" eliminated (51→0), MISS on re-parse −8,
         skipping RelationChange −2. No new warning categories.
         `cache_fetch FAILED` fix (anchor map via TapePayload)
         validated — count unchanged at 1 (pre-existing
         cross-subnet forward ref).
   - [x] Clippy clean: `cargo clippy --all-features --all-targets
         -- -D warnings` passes with 0 errors. 11 fixes applied
         (unused imports, type complexity, clone-on-copy,
         derivable impl, iter style, dead test helpers).
   - [x] `cargo doc --no-deps` clean.
   - [ ] Profile the enriched halo (`sko-[*]-sko` vs old
         `sk-[*]-sk`): measure `evaluate_query` time on
         representative queries. **Deferred** — the large corpus build
         shows no Phase 0 regression; micro-profiling is optional
         follow-up.
   - [ ] Profile owner memo maintenance overhead. **Deferred** —
         no observable regression in that corpus's wall time.
   - [ ] Verify MCP tool response times. **Deferred** — trait
         is now object-safe with one Box allocation per call;
         MCP tools call trait methods directly.

## Testing Requirements

- Each phase must pass `cargo test --features all` before proceeding
- MCP tool migration: verify MCP tool outputs are identical before/after
- Compiler migration: verify rendered HTML is identical before/after
- No consumer should construct `Expression` after Phase 3
- Issue 79 carryover: `tests/codec_test/query_tests.rs` has 12
  integration tests (11 pass, 1 blocked). Once direct evaluation lands
  (Phase 1), `test_owner_traversal_maps_to` must pass — it validates
  owner-role traversal on `{maps_to}` edges, which fails under the
  Expression-lowering path due to Trace contamination. All 12 tests
  passing is a Phase 1 gate.

## Success Criteria

- [x] `BeliefSource` trait has 5 methods: `evaluate`, `submap`,
      `submap_by_bid`, `get_file_mtimes`, `export_beliefgraph`.
      `get_edges`/`get_node` removed in Phase 7d, replaced by
      `lookup_node`/`lookup_edges` free functions.
- [x] ~~`View` is a closed enum, not a trait~~ Superseded: `View` enum
      deleted in Phase 7a, replaced by consumer-side `Instrument` trait.
- [x] `evaluate` populates `QueryPackage` (fused evaluation + view)
- [x] Halo is `sko-[*]-sko {1}` (full neighbor glob including Owner)
- [x] Owner memo on BeliefBase indexes `WEIGHT_OWNED_BY` edges by bref
- [x] `fetch_owned_edges` deleted — owned edges arrive via graph context
- [x] `Expression` type deleted — no code references it (Phase 6c)
- [x] `QuerySpec` is the sole query representation at all layers
- [x] 616 tests pass
- [ ] MCP tools produce identical results
- [ ] Compiled HTML output is identical
- [ ] Design docs (`beliefbase_architecture.md`, `query_model.md`,
      `dag_model.md`) consistent with `QuerySpec` as primitive

## Risks

- **`submap` has a specialized DB implementation**: The recursive path query
  in `db.rs` doesn't decompose into `Expression` evaluation. Retaining
  `submap` on the trait is acceptable — it's a fundamentally different
  operation (path namespace traversal, not graph query). →
  **Mitigation**: Keep `submap` on the trait. Long-term, it could become
  a `QuerySpec` variant with a specialized DB-level override.
- ~~**`BeliefBase::eval_query` override for cache**~~: **Resolved** —
  query caching moved to `evaluate` in Phase 5.
- ~~**WASM boundary**~~: **Resolved** — WASM `extract_node_context` uses
  `BeliefContext<'a>` directly (Phase 5c). `get_context_bulk` was already
  efficient. `query()` will accept `QuerySpec` JSON (Phase 6a, clean break).

### View dispatch: factory, not `Box<dyn View>`

Views don't need dynamic dispatch. The caller knows which view
it wants at construction time (from `:render:` option, MCP tool type, or
viewer toggle). A factory match statement dispatches to concrete types:

```
match spec.view {
    View::Table { sort, params } => TableInstrument::from_params(&params)?.render(&result, source),
    View::List { sort, params }  => ListInstrument::from_params(&params)?.render(&result, source),
    ...
}
```

No `Box<dyn View>` needed. `View::render` takes
`&dyn BeliefSource` (the source is erased, the view is concrete).
The factory pattern gives static dispatch on the view side and
dynamic dispatch on the source side.

### `BeliefSource` as trait object (`Box<dyn BeliefSource>`)

The trait uses boxed futures (`Pin<Box<dyn Future + Send>>`) for
object safety. `McpState` holds `Box<dyn BeliefSource>` directly —
no `McpBeliefSource` wrapper, no `BeliefSourceKind` enum, no match
delegation. The boxing overhead is one heap allocation per call,
negligible compared to query/DB work.

`View::render` takes `&dyn BeliefSource`. The factory pattern
for views (static dispatch on the view side) is
orthogonal — the view is concrete, the source is erased.

## Open Questions

- ~~Should `submap` remain on the trait or become a `QuerySpec` variant?~~
  **Resolved**: `submap` is expressible as `Subject::Bids([net_bid]) +
  Traverse(Section, depth)`. The `net_path_in` refiner migration
  demonstrates this works. The DB can detect the section-traversal
  pattern in `eval` and use its specialized path-table query as an
  optimization (Phase 1.5 / BACKLOG). `submap` can be removed from
  the trait once all callers construct `QuerySpec` directly.
- **Edge property registry**: Edge weight properties (`WEIGHT_SORT_KEY`,
  `WEIGHT_DOC_PATHS`, `WEIGHT_OWNED_BY`, `WEIGHT_LINK_TITLE`) are currently
  stringly-typed constants in `properties.rs`. Guided traversal predicates
  (`path:`, `idx:`, `owned_by:`) need to map query-surface names to payload
  keys with type validation. Should we add a singleton registry (like
  `WALK_CODECS` for codecs) that maps query names → payload keys → expected
  types? This enables parse-time validation and documentation generation.
  Recommend: yes, but scope to a follow-on issue if it delays Phase 1.
- ~~MCP backward compatibility~~: **Resolved** — clean break decision.
  Both MCP and WASM `query` tools accept only `QuerySpec` JSON.
  No backward compatibility shim for `Expression` JSON.

- ~~**`terminal_roles` implementation strategy**~~: **Resolved** —
  `terminal_roles` deleted from `TraversalSpec` in Phase 7c, replaced
  by `TapeFn::Terminal` which computes terminal nodes as a first-class
  tape operation.

- ~~**`BeliefGraph` → `QuerySpec` migration**~~: **Resolved** —
  `to_event_stream` rewritten to use `QueryPackage::balanced` + tape
  in Phase 7f. `BeliefGraph` no longer depends on `Expression` or
  `RelationPred`.

- **Backend capability declaration**: Each `BeliefSource` backend has
  expressivity limits. A `capabilities()` method on the trait would let
  callers (and the query planner) know what a backend can and cannot
  evaluate:
  - `BeliefBase`: full QuerySpec except `TextMatch` (no TF-IDF index;
    requires Issue 66 incremental search index sync)
  - `DbConnection`: full QuerySpec except `TextMatch`; path-table
    acceleration available for Section traversals
  - `BeliefGraph` (via `BeliefBase::from`): traversal and filter only;
    no Subject resolution beyond `Bids`, no `TextMatch`
  The return type could be a struct with boolean flags or an
  `EnumSet<Capability>`. Callers that need unsupported features
  (e.g. `TextMatch`) would fall back to the search index or reject
  the query with a clear error. Design the capability set when
  `TextMatch` support is added (Issue 66).

## References

- Issue 79 — `QuerySpec` types and evaluation infrastructure
- ~~`src/query/expression.rs`~~ — deleted (Phase 6c)
- `src/query/spec.rs` — `QuerySpec`, `TraversalSpec`, `QueryPackage`
- `src/beliefbase/base.rs` — `BeliefBase` `BeliefSource` impl
- `src/beliefbase/context.rs` — `BeliefContext<'a>` (sole context type;
  `OwnedBeliefContext` deleted in Phase 5c)
- `src/db.rs` — `DbConnection` `BeliefSource` impl
- `src/mcp/tools.rs` — MCP tools (fully migrated to `QueryPackage`)
- `src/mcp/state.rs` — `McpBeliefSource` object-safe shim
- `src/codec/builder.rs` — compile-time query consumer (fully migrated)
- `src/codec/compiler.rs` — fully migrated, uses `lookup_node` (Phase 7d)
