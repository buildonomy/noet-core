# Issue 56: PathMap Protocol Observability Gap

**Priority**: MEDIUM
**Estimated Effort**: 3 days (RELATIVE COMPARISON ONLY)
**Dependencies**: Requires Issue 47 fix (merged). Blocks nothing critical.

## Summary

Issue 47 exposed a class of bug that took 3 sessions to locate: a distance-3 causal
chain where stale cross-document Section edges entered `doc_bb` via `push_relation`'s
unchecked neighbourhood injection, corrupted the tokens.md `PathMap` via an order-slot
collision, and caused a downstream panic in Phase 4. The fix was 4 lines. The difficulty
was that no invariant in the system *encoded* why those 4 lines were required, and the
behaviour was not reproducible by any test we could construct without reconstructing
the full multi-round compiler state. This issue documents that gap and frames the
design-level work needed to close it.

## Goals

1. Document why a true regression test for Issue 47 cannot be written with the current
   architecture (no MRE without full compiler pipeline state).
2. Identify the class of bugs this represents and why they are systematically hard to
   catch at the current level of abstraction.
3. Frame the design work needed: a formal protocol model of the
   `BeliefEvent → BeliefBase → PathMap` pipeline that supports property-based or
   model-level testing.

## Investigation Results: The MRE Impossibility

### Attempted approach

The original issue scoped a regression test that would:

1. Parse `cross_doc_tokens.md` to populate `global_bb` with the tokens structure.
2. Seed `session_bb_mut()` with a stale
   `notation_section →[Section, sort_key=1]→ tokens_doc` edge.
3. Parse `cross_doc_notation.md` — triggering the `push_relation` path that would
   propagate the stale edge into `doc_bb` and then into the tokens PathMap.
4. Assert that `tokens.md#character-and-string-literals` survives.

Three structurally different implementations were tried. None produced a test that
failed without the Issue 47 fix.

### Why it cannot be done this way

The bug required `notation_section` to be resident in the tokens PathMap at
**exactly the same order-vector depth** as "Character and String Literals" —
`[NETWORK_SECTION_SORT_KEY, 0, 1]`. That depth only arises when `notation_section` is
treated as a **grandchild** of the network root (a child of `## Literals`) during
PathMap insertion — not as a direct heading child of the root, which is what
`process_relation_update`'s standard insert branch produces when the sink is the
network root node.

The stale edge in the real bug accumulated that specific depth over **multiple parse
rounds** through the full compiler pipeline:

1. First parse of notation: notation_section gets a correct Section edge to
   `notation_doc` at depth 2 under the notation network.
2. Re-parse of notation: `push_relation` fetches `tokens_doc` from `session_bb`
   (which already held the notation structure from round 1). The pre-fix code called
   `union_mut_with_trace` unconditionally for all cached nodes, pulling the notation
   section's full neighbourhood — including its Section edge — into `doc_bb`. The
   edge weight carried `sort_key=1`, which in the notation network referred to
   notation_section's sibling position. When broadcast as a `RelationUpdate` to all
   PathMaps, the tokens PathMap interpreted `sort_key=1` as a child of tokens_doc's
   heading slot, inadvertently placing notation_section at the same slot as
   "Character and String Literals".

This multi-round accumulation cannot be reproduced by a single direct event injection
at the `BeliefBase` level because `process_relation_update`'s insert logic determines
order-vector depth from the *current state of the PathMap* at insertion time, not from
the injected weight alone. The weight's `sort_key` is interpreted relative to the
sink's existing children — so injecting `sort_key=1` with `tokens_doc` as sink places
the node at depth 1 under the heading slot, not at depth 3 where the collision
occurred.

### What the existing test actually covers

`test_cross_doc_stale_section_edge_does_not_corrupt_pathmap` validates that the
**symptom path** (the expected Section nodes all survive after parsing both files) is
stable. It does not fail without the Issue 47 fix because the organic parse in the
test corpus never accumulates the specific stale state — the same guard that fixes the
bug also prevents the test from building up the precondition. The test is valuable but
is not a regression test in the strict sense.

## The Deeper Problem: Protocol Observability

Issue 47 is an instance of a broader class of bug that this codebase is currently
ill-equipped to detect or prevent: **emergent invariant violations in multi-round
stateful protocols**.

The `DocumentCompiler → GraphBuilder → BeliefBase → PathMap` pipeline is a stateful
protocol. Correct system behaviour depends on invariants that span multiple components
and multiple parse rounds:

- `push_relation` must only inject Section-edge neighbourhoods from `session_bb` for
  nodes that belong to the current document's network.
- `PathMap.map` must only contain nodes whose Section parent chain terminates at the
  PathMap's own network root.
- `process_relation_update`'s removal branch must only sweep nodes that were
  legitimately inserted as Section children of the sink.

None of these invariants are encoded anywhere. They are implicit consequences of the
implementation, discovered only when violated. The fix for Issue 47 encodes *one* of
them (the `push_relation` guard), but the `PathMap` layer still has no independent
enforcement — if a foreign node enters the PathMap through any other path, the removal
sweep will corrupt it silently.

### Why integration tests cannot close this gap alone

Integration tests over the full compiler pipeline can validate that a *specific*
corpus produces correct output. They cannot exhaustively validate the protocol because:

1. The relevant state space is the cross-product of all possible round-orderings, all
   possible `session_bb` accumulation histories, and all possible network topologies.
   Integration tests sample this space sparsely by construction.

2. The observable failure (Phase 4 panic) is distance-3 from the root cause
   (unchecked `union_mut_with_trace`). A test that only checks the output cannot tell
   you *which* invariant was violated or *where* in the pipeline the violation
   occurred.

3. The PathMap's internal `map: Vec<(String, Bid, Vec<u16>)>` is private. Tests
   cannot inspect intermediate PathMap state to detect a foreign node before it causes
   a removal sweep. There is no hook between "foreign node inserted" and "removal sweep
   fires" that a test can assert on.

### The structural gap

The system currently lacks:

- A **protocol model** that defines the valid state transitions for the
  `session_bb → push_relation → doc_bb → PathMap` pipeline. Without a model, there
  is no reference to check an implementation against.
- **Boundary enforcement** at the PathMap layer. The PathMap accepts any
  `RelationUpdate` event and processes it. It has no way to distinguish a legitimate
  insertion from a corrupted one, because the invariant that distinguishes them
  (home-network membership) is not encoded in the event or in PathMap's own state.
- **Testable abstraction boundaries**. The pipeline's stages are deeply coupled:
  `push_relation` writes to `missing_structure`, which is merged into `doc_bb`, which
  fires `RelationUpdate` events, which are processed by `PathMap`. There is no clean
  seam where a test can observe or inject state at the protocol level without
  reconstructing the entire compiler context.

## What a Design Solution Would Look Like

This issue does not prescribe an implementation. It names the design work that needs
to happen.

The goal is an abstraction that supports **protocol-level reasoning** about the
`BeliefEvent → BeliefBase → PathMap` pipeline: one that makes invariants explicit,
makes them testable in isolation, and makes violations detectable at the point of
occurrence rather than at the downstream panic.

This is a hard design problem. The pipeline is inherently stateful and multi-round.
A solution will likely need to answer:

- What are the **typed contracts** at each boundary in the pipeline? What events is
  each component permitted to receive, and what state transitions are those events
  permitted to produce?
- Can `PathMap` enforce home-network membership **at insertion time** without a full
  graph walk, or does enforcement require making the graph walk cost acceptable in
  the hot path?
- Is the right abstraction a **formal protocol model** (TLA+/PlusCal, or an equivalent
  Rust-native model), a **session-typed event bus**, or something else entirely?
- Would making `PathMap` aware of its own network's valid node set (a closed-world
  assumption within a network boundary) give it enough information to enforce the
  invariant independently?

The scope of whatever design doc comes out of this is the
`BeliefEvent → BeliefBase → PathMap` pipeline: its valid states, its valid
transitions, and how violations are detected and reported. That design doc is the
prerequisite for any implementation of protocol-level testing.

## Success Criteria

- [ ] A design document exists for the `BeliefEvent → BeliefBase → PathMap` protocol
      that defines valid states and transitions formally enough to support either
      property-based testing or model checking.
- [ ] That design document explicitly encodes the home-network invariant and the
      `push_relation` neighbourhood-injection contract as typed boundaries.
- [ ] The existing organic test
      `test_cross_doc_stale_section_edge_does_not_corrupt_pathmap` has a doc-comment
      that accurately describes what it covers and what it does not (updated to
      reference this issue's findings).

## Risks

- **Design scope creep**: The pipeline touches `DocumentCompiler`, `GraphBuilder`,
  `BeliefBase`, and `PathMap`. A full protocol model could easily balloon into a
  complete architecture redesign. **Mitigation**: Scope the design doc to the
  specific pipeline segment — `push_relation → missing_structure → doc_bb →
  PathMap` — not the full system.

- **No good prior art for this exact problem**: Stateful multi-round document compilers
  with mutable graph backends are not a well-studied domain. TLA+/PlusCal is the
  closest formal tool, but it requires significant investment to get right and the
  translation from protocol model to test harness is not automatic.
  **Mitigation**: Start by writing the invariants as comments and `debug_assert`s,
  then evaluate whether a formal model is warranted based on how many new violations
  emerge from the asserts.

## Open Questions

- Is TLA+/PlusCal the right tool, or is there a lighter-weight option (session types,
  a Rust-native state machine library, an effect system) that covers the important
  cases without the full formalism overhead?

- Should the home-network invariant be enforced at `PathMap` insertion time as a
  hard error in release builds, not just a `debug_assert`? The cost of a foreign node
  is silent data corruption that surfaces as a panic several steps later — a hard
  error at the insertion point would be strictly better for diagnosability. This is a
  decision for the design doc.

- Is the `PathMap.map` field the right level of abstraction for enforcement, or should
  enforcement happen earlier — at the `BeliefBase::process_event` level — where the
  network topology is available and the event origin is known?