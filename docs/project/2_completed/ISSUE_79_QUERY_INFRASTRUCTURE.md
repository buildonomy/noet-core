# Issue 79: Query Infrastructure — QuerySpec, Score, and Evaluator

**Version**: 0.1
**Priority**: HIGH
**Estimated Effort**: 5 days (RELATIVE COMPARISON ONLY)
**Status**: Near-complete (Steps 1–5 done, Step 6 deferred to Issue 82)
**Dependencies**: Blocks Issue 80 (Query Parser), Blocks Issue 81 (`{query}`
Directive). Related to Issue 83 (BeliefSource refactoring — the follow-on
that deletes `Expression` and makes `QuerySpec` the sole primitive).

## Summary

The query model design (`docs/design/query_model.md`) defines a three-component
`QuerySpec` (Subject, Projection, Instrument) that unifies all current query
tools as special cases of a single evaluation pipeline. This issue implements
the core types, evaluation bridge, and table instrument, creating the
programmatic API that the query parser (Issue 80), `{query}` directive
(Issue 81), and viewer (Issue 82) build on.

The evaluation currently lowers `QuerySpec` to the existing `Expression`/
`eval_query` infrastructure. Issue 83 replaces this with direct evaluation
and deletes `Expression`. The `QuerySpec` types and `Instrument` trait from
this issue are stable across that transition — consumers don't change.

## Goals

- Define `QuerySpec`, `ProjectionStep`, `Score`, `NodeFilter`, `TraversalSpec`,
  and `InstrumentConfig` as Rust types
- Implement `QuerySpec` evaluation by lowering to `Expression`/`eval_query`
  (to be replaced by direct evaluation in Issue 83)
- Define `Tape` — per-step intermediate results for composition provenance
- Define `Instrument` trait with `TableInstrument` (4 display modes)
- Define `PropertyPredicate` with TOML-based path resolution and diagnostics
- Replace `NeighborsExpression` with `TraversalSpec` throughout the codebase
- Validate via integration tests against compiled fixtures

## Architecture

### Module structure

```
src/query/
├── mod.rs              # re-exports
├── expression.rs       # Expression, StatePred, BeliefSource trait
│                       # (to be deleted in Issue 83)
├── spec.rs             # QuerySpec types, evaluation bridge
└── instrument/
    ├── mod.rs           # Instrument trait, InstrumentOutput, factory
    └── table.rs         # TableInstrument, 4 display modes
```

### Evaluation flow (current — via Expression bridge)

```
QuerySpec::evaluate(&BeliefSource)
    → to_query() lowers to Query { seed: Expression, traverse: Vec<TraversalSpec> }
    → BeliefSource::eval_query(&Query, all_or_none)
        → eval_unbalanced(seed Expression) → BeliefGraph
        → traversal loops (upstream/downstream/owner)
        → balance
    → post-filter in memory for payload/metadata predicates
    → QueryResult { entries, tape, instrument }
```

### Evaluation flow (future — Issue 83, direct)

```
BeliefSource::eval(&QuerySpec)
    → backend translates QuerySpec directly (DB→SQL, BeliefBase→petgraph)
    → no Expression intermediary
    → QueryResult { entries, tape, instrument }
```

### Key types

- **`QuerySpec`** — Subject + `Vec<ProjectionStep>` + InstrumentConfig
- **`Subject`** — `Anchor(String)`, `Bids(Vec<Bid>)`, `CorpusWide`, `Implicit`
- **`ProjectionStep`** — `Filter(NodeFilter)`, `Traverse(TraversalSpec)`,
  `Compose(Composition)`
- **`NodeFilter`** — `Predicate(PropertyPredicate)`, `TextMatch { path, query }`
- **`PropertyPredicate`** — `{ path: PropertyPath, op: CompareOp, value }`
  resolved via TOML serialization of `BeliefNode`
- **`TraversalSpec`** — `{ input_roles, kind_filter, output_roles, depth }`
  used directly in `Query.traverse` (replaces `NeighborsExpression`)
- **`Tape`** — `Vec<TapeEntry>` recording per-step intermediate results
- **`InstrumentConfig`** — `{ render: RenderMode, sort: SortSpec, params: Table }`
- **`Instrument`** trait with `render(&QueryResult, &BeliefGraph)`
- **`TableInstrument`** — Depth0, Columns, EdgeCount, MapsTo display modes

## Implementation Steps

1. Define core types (1 day) — **COMPLETE**
   - [x] All types in `src/query/spec.rs`
   - [x] `PropertyPath` resolution via TOML serialization
   - [x] `PropertyPredicate::evaluate()` returns `PredicateResult` with
         diagnostics
   - [x] `SortSpec: FromStr` with case-insensitive parsing
   - [x] `InstrumentConfig` with opaque `params: toml::Table`

2. Evaluation bridge (1 day) — **COMPLETE**
   - [x] `QuerySpec::evaluate(&BeliefSource)` via Expression lowering
   - [x] `TraversalSpec` replaces `NeighborsExpression` throughout
   - [x] Owner traversal via `RelationPred::OwnedBy` in `eval_query`
   - [x] `Compose(And/Or/Not)` lowered to `Dyad(SetOp::*)`
   - [x] Tape recording for composition steps

3. Instrument implementation (0.5 day) — **COMPLETE**
   - [x] `Instrument` trait with `render()` method
   - [x] `TableInstrument` with 4 display modes
   - [x] Tape-aware A/B rendering for gap analysis
   - [x] `create_instrument()` factory

4. Structural improvements — **COMPLETE**
   - [x] Module restructuring
   - [x] `Subject::Bids` unification
   - [x] `CompositionOp::Not` (boolean naming)
   - [x] `NodeFilter` simplified (no inline conjunction)
   - [x] `ResolveResult` with diagnostics
   - [x] `PredicateResult` with diagnostics
   - [x] `PathSegment::parse` returns `Result`
   - [x] `thiserror` for all error types
   - [x] Audit: silent fallbacks logged, panics → Results

5. Integration tests (1 day) — **11/12 PASS**
   - [x] `tests/codec_test/query_tests.rs` created, 12 tests
   - [x] Section traversal matches `BeliefSource::submap` results
   - [x] Schema/kind predicates return correct node sets
   - [x] Pragmatic traversal on `{uses}` edges
   - [ ] Owner traversal on `{maps_to}` edges — **FAILS** (Trace
         contamination from Expression lowering; fix requires Issue 83
         direct evaluation)
   - [x] `Composition(And)` and `Composition(Not)` semantics
   - [x] `TableInstrument` renders live results (Depth0, EdgeCount,
         MapsTo, render_rows)
   - [x] `QuerySpec` JSON serde round-trip

6. Viewer validation — **DEFERRED to Issue 82**
   Viewer refactoring requires the `Instrument` enum and fused `eval`
   from Issue 83. Moved to Issue 82 (viewer enhancements), which
   depends on Issue 83.

## Testing Requirements

- 703 tests currently pass (`cargo test --features all`)
- Integration tests use `tests/network_1/` fixtures
- Tests construct `QuerySpec` programmatically (no parser dependency)
- Verify result BID sets by count and structural position

## Success Criteria

- [x] `QuerySpec` types are stable public API
- [x] Evaluation produces correct results for standard projection types
      (11/12 integration tests pass; owner traversal blocked on Issue 83)
- [x] `TableInstrument` renders correct HTML for all 4 display modes
- [x] Integration tests pass against compiled fixtures (11/12)
- [x] All 700+ tests pass
- [ ] Owner traversal test — blocked on Issue 83 direct evaluation
- [ ] Viewer validation — deferred to Issue 82

## Risks

- **Trace contamination in Expression lowering**: `evaluate_expression`
  adds Trace endpoint nodes for referential integrity. These leak through
  `Intersection`/`Difference` boundaries, producing spurious nodes in
  composition results. Owner-role traversal (`o-pragmatic-k`) is affected
  — `test_owner_traversal_maps_to` fails. Standard source→sink and
  sink→source traversals are NOT affected (11/12 tests pass). →
  **Mitigation**: Issue 83 direct evaluation eliminates this by operating
  on `BTreeSet<Bid>` instead of `BeliefGraph`, never touching Trace.
  P0 use case (`k-pragmatic-s(1)`) works correctly today.
- **Payload predicates fall back to `StatePred::Any`**: Fetches full
  corpus then filters in memory. → **Mitigation**: Logged with
  `tracing::debug!`. Issue 83 direct evaluation eliminates this.
- **`TextMatch` returns error**: Needs Issue 66 TF-IDF integration.

## Open Questions

None remaining. WASM and viewer validation deferred to Issue 82.

## References

- `docs/design/query_model.md` §3–§7 — formal model
- `src/query/spec.rs` — QuerySpec types and evaluation
- `src/query/instrument/table.rs` — TableInstrument
- `src/query/expression.rs` — Expression, BeliefSource (to be deleted in 83)
- Issue 80 — query parser (depends on this)
- Issue 81 — `{query}` directive (depends on this + 80)
- Issue 82 — viewer enhancements (depends on this + 80)
- Issue 83 — BeliefSource refactoring (deletes Expression, direct evaluation)
