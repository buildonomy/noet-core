# Issue 77: Proximity-Based `id://` Resolution

**Version**: 0.1
**Priority**: MEDIUM
**Estimated Effort**: 3 days (needs design doc first)
**Dependencies**: None

## Summary

`get_from_id` in `pathmap.rs` walks subnets via `BTreeSet<Bid>::iter().find_map()`,
returning the first match in UUID sort order. When two nodes in different networks
share the same `id` (e.g., `"telemetry"` as both a CMake directory node and an
Architecture design doc), the result is nondeterministic from the user's perspective
— whichever network's BID sorts first wins. This produces silent, hard-to-diagnose
spurious edges. The fix is to prefer structurally "closer" networks when resolving
ambiguous `id://` references.

**Tier 1 implemented**: `push_relation` in `builder.rs` now narrows default-net
`Id` keys to the document's home network (innermost heading==1 stack entry) before
`cache_fetch`, with repo-wide fallback on miss. This handles the common case
(id collision across distant networks). Remaining work: apply the same logic in
`pathmap.rs` directly (for non-builder callers), add diagnostics on ambiguity,
and extend to `get_from_title`/`get_from_title_regex`.

## Goals

- Eliminate nondeterministic `id://` resolution when multiple networks contain nodes
  with the same `id`
- Prefer matches in structurally closer networks to the referring node's home network
- Apply the same proximity logic to `get_from_title` and `get_from_title_regex`,
  which share the same `find_map` over `subnets` pattern
- Preserve current performance for the common case (single match, no ambiguity)
- Emit diagnostics when ambiguity is detected (even if resolved by proximity)

## Architecture

The current `find_map` short-circuits on BID sort order, which has no semantic
relationship to network structure. The fix replaces this with proximity-aware
ranking:

1. **Distance metric**: Section-edge hops from the referring node's home network to
   each candidate's home network. Networks within the same subnet tree are "closer"
   than networks in distant branches.

2. **Resolution strategy**: Collect all matches across subnets, rank by distance,
   select the closest. This replaces the current early-exit `find_map`.

3. **Tie-breaking**: When multiple candidates share the same distance, emit a
   warning diagnostic and fall back to BID sort order (preserving determinism).

4. **Scope**: `get_from_id`, `get_from_title`, `get_from_title_regex`, and
   potentially `get_from_path` — all use the same subnet-walking pattern.

5. **Performance**: The `filter_states` calls in `base.rs` (~L2930-2940) use these
   lookups. The common case (unique `id` across the corpus) should remain fast,
   possibly via an early exit when only one match is found.

## Implementation Steps

1. Design doc: proximity metric and resolution semantics (0.5 days)
   - [ ] Define distance metric (section-edge hops vs. subnet containment depth)
   - [ ] Decide "prefer local then widen" vs. "collect all, rank by distance"
   - [ ] Specify tie-breaking behavior and diagnostic severity
   - [ ] Evaluate performance impact on `filter_states` hot path

2. Refactor subnet-walking helpers in `pathmap.rs` (1 day)
   - [ ] Extract shared "walk subnets and collect matches" logic
   - [ ] Add `referring_bid` (or home-network BID) parameter to resolution functions
   - [ ] Implement proximity ranking with early exit on unique match
   - [ ] Apply to `get_from_id`, `get_from_title`, `get_from_title_regex`

3. Diagnostics and testing (1 day)
   - [ ] Emit warning when ambiguity is detected (even if resolved)
   - [ ] Add test with two networks containing same `id`, verify closest wins
   - [ ] Add test for tie-breaking behavior
   - [ ] Verify no performance regression on large corpora (production scale, tens of thousands of nodes)

4. Evaluate `get_from_path` (0.5 days)
   - [ ] Determine if `get_from_path` has the same ambiguity risk
   - [ ] Apply proximity logic if warranted, or document why not

## Testing Requirements

- Two-network fixture where both contain a node with `id: "shared_name"` — verify
  the structurally closer network wins
- Three-network fixture with equidistant candidates — verify warning emitted and
  deterministic tie-break
- Single-match case — verify no performance regression vs. current `find_map`
- Integration test against a real multi-network corpus confirming no spurious edges

## Success Criteria

- [ ] `id://` resolution prefers structurally closer networks over BID sort order
- [ ] Ambiguous resolutions produce a diagnostic warning
- [ ] `get_from_title` and `get_from_title_regex` use the same proximity logic
- [ ] No measurable performance regression for unambiguous lookups
- [ ] Design doc approved before implementation begins

## Risks

- **Performance regression on large corpora**: Current `find_map` bails early;
  ranking requires checking all subnets. → **Mitigation**: Early exit when first
  subnet scan finds exactly one match; only engage ranking when multiple matches
  exist.
- **Distance metric complexity**: Section-edge hop counting may be expensive or
  poorly defined for deeply nested subnet trees. → **Mitigation**: Start with
  simple "same network > parent network > other" tiers before implementing full
  hop counting.

## Open Questions

- Should the distance metric use section-edge hops, or a simpler tier system
  (local > parent > grandparent > other)?
- Is `get_from_path` affected, or does path-based resolution already have enough
  structural context to avoid ambiguity?
- Should ambiguity warnings be suppressible (e.g., via a `net:` qualifier on the
  `id://` reference)?

## Related

Tier 1 (home-network-first with repo-wide fallback) was implemented in
`builder.rs::push_relation` as part of the spurious xlsx pragmatic edges
investigation (session 2026-05-05). This issue tracks the remaining tiers:
PathMap-level proximity, diagnostics, and `get_from_title` parity.
