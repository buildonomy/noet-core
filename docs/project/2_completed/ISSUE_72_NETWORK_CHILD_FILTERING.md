# Issue 72: Network Child Filtering (Whitelist / Blacklist)

**Priority**: HIGH
**Estimated Effort**: 2 days (RELATIVE COMPARISON ONLY)
**Dependencies**: Requires Issue 68 (Two-Registry Codec Architecture)
**Blocks**: Issue 67 Phase 2 (YAML sub-codecs benefit from explicit network scoping)

## Summary

Networks need the ability to declare which child files they accept, using per-network
whitelist and blacklist glob patterns in `index.md` frontmatter. This is implemented by
extending `NetworkCodec` in two phases: `NetworkCodec::parse()` registers accepted plain
files in `CLAIM_MAP` during Phase 1 (before Phase 2 dispatch); `prepare_proto_relations`
builds `Section` edges and handles claiming/rejection for all children (including subnet
dirs). Rejected children are registered with `CLAIM_MAP.reject()` (a `None` sentinel), so
`parse_one_path` routes them to `UnclaimedDataCodec` + `ParseDiagnostic::info` without
producing any nodes.

## Problem Statement

Currently `net_dir_partition` includes any file matched by `CODECS ∪ WALK_CODECS`
(after Issue 68). There is no per-network mechanism to exclude files that happen to
be present in a directory but are not semantically part of that network — for example,
generated output files, vendored assets, or build artifacts that live alongside source.
Without filtering, every such file either produces unwanted nodes or requires
corpus-wide `WALK_CODECS` configuration that cannot express per-network intent.

## Goals

1. `index.md` frontmatter supports `whitelist` and `blacklist` as arrays of glob
   patterns (relative to the network directory).
2. Filter semantics are well-defined for all four combinations of empty/non-empty
   lists (see Architecture).
3. Accepted children are claimed in `CLAIM_MAP` and receive `Section` edges; rejected
   children are neither claimed nor edged.
4. Rejected children emit `ParseDiagnostic::info`, not `warning` or `error`.
5. Filtering is implicitly scoped: a subnet whose directory is rejected by its parent
   network is never claimed, never parsed, and its own filters never run.
6. ~~`MdWalkCodec` is added to `WALK_CODECS`~~ — **done in Issue 68 implementation**.

## Architecture

### Frontmatter Schema

```toml
# index.md frontmatter (TOML/YAML/JSON — existing auto-detection applies)
whitelist = ["src/**/*.md", "docs/**/*.md"]   # optional; omit or [] = accept all
blacklist = ["generated/**", "**/scratch.*"]  # optional; omit or [] = reject nothing
```

Patterns are globs relative to the network directory. Standard `*` and `**` operators.
Patterns are anchored to the network directory root: `generated/**` matches
`<net_dir>/generated/foo.yaml` but not `<other_net>/generated/foo.yaml`.

Uses the `globset` crate (`build_glob_set`, `apply_child_filter` in `src/codec/network.rs`).

### Filter Semantics

| whitelist | blacklist | result for a candidate path              |
|-----------|-----------|------------------------------------------|
| empty     | empty     | accept (current behaviour, default)      |
| empty     | non-empty | accept unless blacklist matches          |
| non-empty | empty     | accept only if whitelist matches         |
| non-empty | non-empty | accept if whitelist matches AND blacklist does not match |

### Call-Site: `NetworkCodec::parse()` (Phase 1) and `prepare_proto_relations` (Phase 2 setup)

Claiming happens in two complementary call sites:

**`NetworkCodec::parse()` — primary claim site for plain files (Phase 1)**

`parse()` fires during Phase 1 when the network's `index.md` is processed — before
any Phase 2 dispatch. It reads `whitelist`/`blacklist` from `current.document`, calls
`proto_index.children_of(network_dir)` to get the candidate child list, and iterates
over plain files only (subnet dirs are skipped — they parse themselves when their own
`index.md` is processed). This is the site where the `CLAIM_MAP` entries that govern
Phase 2 dispatch are registered.

**`prepare_proto_relations` — Section edge builder and subnet/all-child claiming**

`prepare_proto_relations` fires from `initialize_stack` during Phase 2 ancestor stack
building. It reads the same `whitelist`/`blacklist`, applies the same filter, and:
- For accepted children: calls `CLAIM_MAP.claim()` and pushes `Section` edges onto
  `proto.upstream`.
- For rejected children: calls `CLAIM_MAP.reject()` and emits `ParseDiagnostic::info`.

Note: the diagnostics vector in `prepare_proto_relations` is currently dropped (no
return channel in the signature). A TODO comment marks this for a future follow-on.

```
Phase 1 — parse_sequential, network dirs depth-first:
  parse_one_path(net_dir/index.md):
    codec.parse(content, current, diagnostics, proto_index):
      → NetworkCodec::parse() reads whitelist/blacklist from current.document
      → proto_index.children_of(network_dir) → candidate child list
      → for each child (plain files only; subnet dirs skipped — they parse themselves):
          if accept(child):
            CLAIM_MAP.claim(abs_path, factory)     ← registered here, Phase 1
          else:
            CLAIM_MAP.reject(abs_path)             ← rejection sentinel
            diagnostics.push(ParseDiagnostic::info(...))

Phase 2 — initialize_stack / parse_one_path:
  codec.prepare_proto_relations(proto, net_dir, child_paths):
    → reads whitelist/blacklist from proto.document
    → for each child (including subnet dirs):
        if accept(child):
          CLAIM_MAP.claim(abs_path, factory)       ← also claimed here
          push Section edge onto proto.upstream
        else:
          CLAIM_MAP.reject(abs_path)
          diagnostics (currently dropped — no return channel)

  parse_one_path for each file in ordered_paths():
    branch 1: CLAIM_MAP.get(path)     → claimed codec → parse normally
    branch 2: CLAIM_MAP.is_rejected() → UnclaimedDataCodec + ParseDiagnostic::info
    branch 3: WALK_CODECS.should_track(), no claim → info, UnclaimedDataCodec
    branch 4: neither                 → process_asset (binary, unchanged)
```

### `ClaimMap` Reject Sentinel

`CLAIM_MAP.reject(path)` stores `None` as a sentinel distinct from "not yet seen".
`parse_one_path` checks `claim_map.is_rejected()` as branch 2 of the four-branch
dispatch, routing rejected files to `UnclaimedDataCodec` + `ParseDiagnostic::info`
without reaching `CODECS.path_get`. This is the mechanism by which explicitly filtered
`.md` files are prevented from being parsed via the old `CODECS` fallback.

### Scoping Guarantee

Implicit, emergent from the claim graph:

- A subnet directory rejected by its parent's filter is never claimed.
- An unclaimed subnet directory reaches `parse_one_path` without a `Section` edge from
  its parent, so it is never enqueued for Phase 2 parsing.
- Therefore a rejected subnet's own `prepare_proto_relations` never runs, and its
  children are never claimed. The entire subtree is excluded.

No explicit parent-propagation logic is needed.

### Diagnostic Levels

| Situation | Level |
|-----------|-------|
| Child explicitly rejected by whitelist/blacklist | `info` |
| Tracked file with no claiming network at all | `info` |
| Malformed glob pattern in frontmatter | `warning` (skips pattern, continues) |

All three cases continue to completion with no `BuildonomyError`.

## Deviations from Original Spec

| Item | Original | Actual |
|------|----------|--------|
| Claiming call site | `prepare_proto_relations` only | Both `NetworkCodec::parse()` (Phase 1, plain files) and `prepare_proto_relations` (Phase 2 setup, all children) |
| `DocCodec::parse` signature | Unchanged | Gained `proto_index: &ProtoIndex` (Issue 68 deviation) |
| Rejection mechanism | No claim, no edge | `CLAIM_MAP.reject()` sentinel + four-branch dispatch in `parse_one_path` |
| `MdWalkCodec` | Added in this issue | Added in Issue 68 implementation |
| `prepare_proto_relations` diagnostics | Surfaced to caller | Currently dropped — no return channel in signature (TODO noted in source) |
| `with_claim_map` isolation | Full test isolation | Constructor exists; full isolation blocked — `NetworkCodec::parse()` writes to global `CLAIM_MAP` via the `parse()` call chain, threading it through would require a larger refactor |
| Subnet rejection test | Required | Not yet written — outstanding |

## Implementation Steps

1. **`MdWalkCodec`** — `src/codec/mod.rs` (0.25 day)
   - [x] Define `MdWalkCodec` struct and `WalkCodec` impl (extension `"md"`)
   - [x] Register in `WALK_CODECS` static initializer alongside existing codecs
   - [x] Unit test: `WALK_CODECS.should_track` returns true for `foo.md`, false for `foo.rs`
   - **Note**: delivered in Issue 68 implementation, not this issue.

2. **Glob filter helper** — `src/codec/network.rs` (0.25 day)
   - [x] `build_glob_set` and `apply_child_filter` implemented in `network.rs` (L17–56)
   - [x] `globset` added to `Cargo.toml`
   - [x] Whitelist/blacklist arrays read from `proto.document` in `prepare_proto_relations` and from `current.document` in `NetworkCodec::parse()`; `ParseDiagnostic::warning` per malformed pattern
   - [x] Unit tests for all four whitelist/blacklist combinations (`test_apply_child_filter_*`)
   - [x] Unit test: subnet dir path matched via `index.md` form (`test_apply_child_filter_subnet_index_md_form`)

3. **Claiming in `NetworkCodec::parse()` and `prepare_proto_relations`** — `src/codec/network.rs` (0.5 day)
   - [x] `NetworkCodec::parse()` (L250–366): plain-file claiming/rejection in Phase 1 via `proto_index.children_of()`; subnet dirs skipped
   - [x] `prepare_proto_relations` (L136–248): all-child claiming/rejection + Section edge construction; diagnostics currently dropped (noted as TODO)
   - [x] Default (empty filters): all children claimed — identical observable behaviour to pre-issue baseline
   - **Note**: original spec placed claiming only in `prepare_proto_relations`; actual implementation adds a Phase 1 claim site in `parse()` for plain files.

4. **`parse_one_path` four-branch dispatch + info diagnostic** — `src/codec/compiler.rs` (0.25 day)
   - [x] Four-branch dispatch: claimed → codec; `is_rejected()` sentinel → `UnclaimedDataCodec` + `ParseDiagnostic::info`; tracked+unclaimed → info + `UnclaimedDataCodec`; neither → asset
   - [x] `ParseDiagnostic::info` (not `warning`) for rejected and unclaimed-tracked files

5. **Integration tests** — `tests/codec_test/filter_tests.rs` (0.5 day)
   - [x] Blacklist filtering of plain files (`test_blacklisted_files_produce_no_nodes`): accepted doc present, blacklisted docs absent
   - [x] Info diagnostics emitted for filtered files (`test_blacklisted_files_emit_info_diagnostics`): no errors, info mentions filtered paths
   - [x] Parse 2 stability (`test_filter_parse_2_stable`): blacklisted files absent across both parse runs
   - [x] `with_claim_map` constructor smoke test (`test_with_claim_map_constructor_smoke`)
   - [ ] Subnet rejection: parent blacklists a subnet dir; subnet and all descendants produce no nodes — **not yet written**

## Testing Requirements

- `apply_child_filter`: exhaustive table of the four filter-combination cases —
  covered by `test_apply_child_filter_*` unit tests in `network.rs`
- Malformed glob pattern: `build_glob_set` emits `ParseDiagnostic::warning`, continues
  with remaining valid patterns, does not propagate `BuildonomyError`
- Default (no frontmatter keys): `test_proto_for_upstream_matches_network_codec_proto`
  still passes without modification — primary regression guard ✓
- Subnet exclusion: parent blacklist of a subnet dir prevents all descendant nodes —
  **not yet written; outstanding**
- `CLAIM_MAP` state isolation: `DocumentCompiler::with_claim_map` constructor exists
  and is smoke-tested. Full per-test isolation is not achievable without threading the
  `ClaimMap` through `NetworkCodec::parse()` — deferred to a future issue.

## Success Criteria

- [x] `whitelist` and `blacklist` frontmatter keys are read from `index.md` without
  error; absent keys default to empty (accept-all / reject-nothing).
- [x] All four filter combinations produce correct accept/reject decisions.
- [x] Accepted children appear in `proto.upstream` as `Section` edges and are claimed
  in `CLAIM_MAP`; rejected children have neither.
- [x] Rejected children (and unclaimed-tracked files generally) emit
  `ParseDiagnostic::info` and zero `BeliefBase` nodes.
- [ ] A blacklisted subnet directory produces no nodes for itself or any descendant —
  correct by design (rejected subnet is never parsed) but **not tested**; outstanding.
- [x] All existing `ProtoIndex` and `NetworkCodec` tests pass unmodified.
- [x] WASM build passes (`CLAIM_MAP` stub behaviour is unchanged).

## Risks

- **Risk: `globset` not yet a direct dependency** — resolved; added explicitly to
  `Cargo.toml`.

- **Risk: `CLAIM_MAP` global state pollution between tests** — partially mitigated.
  `DocumentCompiler::with_claim_map` constructor exists. Full isolation blocked until
  `ClaimMap` is threaded through `NetworkCodec::parse()`.

- **Risk: default-claim path changes observable codec dispatch** — mitigated. Default
  claim uses `CODECS.path_get(child)` as the factory source in both call sites,
  guaranteeing the same codec as before. Regression guard passes.

- **Risk: `prepare_proto_relations` diagnostics silently dropped** — known limitation.
  Glob-build warnings and filter info messages from `prepare_proto_relations` are
  collected but discarded (no return channel). Surfacing them requires a signature
  change; deferred to a follow-on issue.

## Open Questions

- ~~**Glob anchoring**~~: resolved — patterns are network-relative (portable across
  machines; anchored to `<net_dir>`).

- ~~**Subnet dir paths in filter**~~: resolved — subnet directories are matched using
  their `index.md` path (`<subnet_dir>/index.md`) in `prepare_proto_relations`; plain
  files use their full network-relative path. `NetworkCodec::parse()` skips subnet dirs
  entirely (they parse themselves).

- ~~**`globset` dependency**~~: resolved — added as an explicit dependency in
  `Cargo.toml`.

- **Subnet rejection integration test** — outstanding. The scoping guarantee is correct
  by design but is not covered by `filter_tests.rs`. Should be added as a follow-on.

- **`prepare_proto_relations` diagnostics return channel** — outstanding. Currently
  dropped. Surfacing them requires a signature change across all `DocCodec` implementors.

## References

- Issue 68: Two-Registry Codec Architecture — prerequisite; provides `WALK_CODECS`,
  `CLAIM_MAP`, `UnclaimedDataCodec`, `MdWalkCodec`, and `DocCodec::parse` signature
  with `proto_index` parameter
- Issue 67: Horizon FSW Source Codec — secondary consumer; benefits from scoped
  network filtering of YAML files
- `src/codec/network.rs` — `NetworkCodec::parse()` (L250–366) — primary Phase 1
  claim site for plain files; `prepare_proto_relations` (L136–248) — Section edge
  builder and all-child claim/reject site; `build_glob_set` / `apply_child_filter`
  (L17–56)
- `src/codec/proto_index.rs` — `net_dir_partition` (L129–280), `ProtoIndex::proto_for`
  (L643–701) — call-site context
- `src/codec/compiler.rs` — `parse_one_path` (~L1491) — four-branch dispatch,
  `is_rejected()` sentinel check
- `src/codec/mod.rs` — `WALK_CODECS`, `CODECS`, `CodecFactory` — registration patterns
- `tests/codec_test/filter_tests.rs` — integration tests for blacklist filtering,
  info diagnostics, parse-2 stability, and `with_claim_map` smoke test