# Issue 68: Two-Registry Codec Architecture (`WALK_CODECS` + `CLAIM_MAP`)

> **Context for readers**: This issue was designed in tandem with Issue 67 (Horizon FSW
> Source Codec). The ordering model described here is grounded in the actual
> `parse_sequential` and `parse_one_path` execution structure in `compiler.rs`. Read the
> "Ordering Guarantee" and "The Asset-Branch Problem" sections carefully before implementing.

**Priority**: HIGH
**Estimated Effort**: 4 days (RELATIVE COMPARISON ONLY)
**Dependencies**: None (infrastructure; unblocks Issue 67 Phase 2)
**Blocks**: Issue 67 Phase 2 (YAML sub-codecs), any future codec that owns non-Markdown structured data

## Summary

The current codec dispatch pipeline has a single registry (`CODECS`) that conflates two distinct concerns: *walk-time visibility* (should this file appear in `ProtoIndex` child lists?) and *parse-time dispatch* (which codec owns this file and should parse it?). This works for Markdown because the same extension answers both questions. It breaks for structured data files (YAML, TOML, CSV, Protobuf, etc.) where a primary codec must first parse an orchestrating document before it can determine which data files it owns and which codec variant to apply to each.

This issue implements a two-registry architecture that separates the two concerns cleanly. One addition was made to `DocCodec::parse` (a `proto_index: &ProtoIndex` parameter) and the bare `.md` extension entry was removed from `CODECS` — see "Deviations from Original Spec" below.

## Problem Statement

`ProtoIndex::build()` calls `net_dir_partition()`, which filters every file in the repo through:

```noet-core/docs/project/ISSUE_68_TWO_REGISTRY_CODEC_ARCHITECTURE.md#L1-1
if CODECS.get(&p_ap_file).is_some() { Some(p) } else { None }
```

Files with unregistered extensions are silently dropped from every network's child list before any codec's `parse()` has run. The claiming codec only fires after the walk is complete — too late to inject additional paths.

Even if a file somehow reached `parse_one_path` without being in `CODECS`, it would be routed to `process_asset` (the binary-asset path), which is the wrong treatment for structured text data. The fix must intercept at both points.

This is not a problem specific to any one codec. Any codec that owns structured data files (YAML schemas, TOML manifests, JSON schemas, CSV tables, `.proto` definitions) faces the same constraint. The fix must be general.

## Goals

1. Add `WALK_CODECS`: a lightweight global registry answering "should this file be tracked?" at walk time, independently of which codec will parse it.
2. Add `CLAIM_MAP`: a global path→codec registry populated at parse time by owning codecs, giving per-file dispatch precision that extension matching cannot provide.
3. Modify `net_dir_partition` to consult `WALK_CODECS` in addition to `CODECS`.
4. Modify `parse_one_path` to check `CLAIM_MAP` before falling back to `CODECS`.
5. Minimise changes to `DocCodec` trait signatures — one addition only: `parse()` gains a `proto_index: &ProtoIndex` parameter so network codecs can register claims during Phase 1.
6. Provide an `UnclaimedDataCodec` that gracefully handles tracked-but-unclaimed files.

## Architecture

### The Two Registries

```noet-core/docs/project/ISSUE_68_TWO_REGISTRY_CODEC_ARCHITECTURE.md#L1-1
┌─────────────────────────────────────────────────────────────────┐
│  ProtoIndex::build()  ←  net_dir_partition (WalkDir pass)       │
│                                                                 │
│  File inclusion filter:                                         │
│    CODECS.get(path).is_some()          ← existing (Markdown,   │
│    || WALK_CODECS.should_track(path)   ← NEW     .md, .yaml, …)│
└─────────────────────────────────────────────────────────────────┘
                          │
                          │  files now visible in ordered_paths()
                          ▼
┌─────────────────────────────────────────────────────────────────┐
│  parse_sequential Phase 1: network dirs (index.md files)        │
│                                                                 │
│  NetworkCodec::parse() reads frontmatter, calls                 │
│  proto_index.children_of() to discover child paths, then:      │
│    CLAIM_MAP.claim(abs_path, SubCodecA factory)                 │
│    CLAIM_MAP.claim(abs_path, SubCodecB factory)                 │
│    CLAIM_MAP.reject(abs_path)  ← explicit exclusion sentinel    │
│                                                                 │
│  Note: prepare_proto_relations still builds Section edges on    │
│  the IRNode but no longer registers CLAIM_MAP entries.          │
└─────────────────────────────────────────────────────────────────┘
                          │
                          │  claims now registered
                          ▼
┌─────────────────────────────────────────────────────────────────┐
│  parse_sequential Phase 2: file children (DFS from ordered_paths)│
│                                                                 │
│  parse_one_path codec lookup (four-branch):                     │
│    1. CLAIM_MAP.get(abs_path)      ← claimed codec (specific)   │
│    2. CLAIM_MAP.is_rejected(path)  ← explicit filter sentinel   │
│    3. CODECS.path_get(path)        ← extension/stem (existing)  │
│    4. WALK_CODECS.should_track()   ← unclaimed tracked          │
│       → UnclaimedDataCodec + info diagnostic                    │
│    5. neither → process_asset                                   │
└─────────────────────────────────────────────────────────────────┘
```

> **Note on `.md` dispatch**: As part of this implementation, the bare `.md` extension
> entry was removed from `CODECS`. Plain `.md` files are now dispatched exclusively via
> `CLAIM_MAP` (or `MdWalkCodec` → unclaimed path). `index.md` remains registered by
> stem+extension in `CODECS`. A latent bug in `CodecMap::get`'s extension-only fallback
> (which matched stem-constrained entries on extension alone, causing `README.md` to
> match `NetworkCodec` via the extension-only slot) was also fixed.

### The Asset-Branch Problem

`parse_one_path` in `compiler.rs` has a two-branch dispatch after reading the file bytes:

```noet-core/docs/project/ISSUE_68_TWO_REGISTRY_CODEC_ARCHITECTURE.md#L1-1
// Current code — the only gate before asset dispatch:
if CODECS.path_get(&file_path).is_none() {
    // Routes to process_asset() — wrong for YAML/header structured text files
    let result = builder.process_asset(...).await;
    return (path, result);
}
// Only reaches here for registered-codec files
```

The fix adds a `CLAIM_MAP.get()` check **before** the `CODECS.path_get().is_none()` test,
a `CLAIM_MAP.is_rejected()` branch, and a `WALK_CODECS.should_track()` guard that intercepts
the remaining unclaimed-but-tracked files before they reach `process_asset`:

```noet-core/docs/project/ISSUE_68_TWO_REGISTRY_CODEC_ARCHITECTURE.md#L1-1
// After this issue — four-branch dispatch:
let claimed = CLAIM_MAP.get(&file_path);
let rejected = CLAIM_MAP.is_rejected(&file_path);

match (claimed, rejected) {
    (Some(factory), _) => { /* codec document path — use factory() */ }
    (_, true) => {
        // Explicitly filtered by owning codec — route to UnclaimedDataCodec + info.
        return unclaimed_result(path, "rejected by claim map");
    }
    _ => {
        let codec_factory = CODECS.path_get(&file_path);
        match codec_factory {
            Some(factory) => { /* existing registered codec path */ }
            None if WALK_CODECS.should_track(&file_path) => {
                // Tracked but unclaimed — emit info diagnostic, produce no nodes.
                return unclaimed_result(path, "tracked but not claimed");
            }
            None => {
                // Genuine asset (image, PDF, etc.) — existing process_asset path.
                let result = builder.process_asset(...).await;
                return (path, result);
            }
        }
    }
}
```

The four-branch dispatch is the core code change to `parse_one_path`. Everything else
(`WalkCodec` trait, `ClaimMap`, `UnclaimedDataCodec`) exists to support it.

### `WalkCodec` trait

```noet-core/docs/project/ISSUE_68_TWO_REGISTRY_CODEC_ARCHITECTURE.md#L1-1
/// Lightweight trait for walk-time file visibility decisions.
///
/// Implementations must be cheap — no file I/O, no content sniffing.
/// Path-based checks only.
pub trait WalkCodec: Send + Sync {
    /// Return true if this file should be included in ProtoIndex child lists.
    fn should_track(&self, path: &Path) -> bool;
}
```

### `WalkCodecMap`

```noet-core/docs/project/ISSUE_68_TWO_REGISTRY_CODEC_ARCHITECTURE.md#L1-1
pub struct WalkCodecMap(Arc<RwLock<Vec<Box<dyn WalkCodec>>>>);

impl WalkCodecMap {
    pub fn create() -> Self { ... }

    /// Register a walk codec. Multiple walk codecs may track the same
    /// extension; `should_track` is true if ANY registered codec returns true.
    pub fn register(&self, codec: Box<dyn WalkCodec>) { ... }

    /// True if any registered WalkCodec claims this path.
    pub fn should_track(&self, path: &Path) -> bool { ... }
}

pub static WALK_CODECS: Lazy<WalkCodecMap> = Lazy::new(WalkCodecMap::create);
```

Concrete walk codecs registered at startup in `noet-core`:

```noet-core/docs/project/ISSUE_68_TWO_REGISTRY_CODEC_ARCHITECTURE.md#L1-1
/// Tracks all .md / .markdown files (plain Markdown, not network index).
pub struct MdWalkCodec;
impl WalkCodec for MdWalkCodec {
    fn should_track(&self, path: &Path) -> bool {
        matches!(path.extension().and_then(|e| e.to_str()), Some("md" | "markdown"))
    }
}

/// Tracks all .yaml / .yml files.
pub struct YamlWalkCodec;
impl WalkCodec for YamlWalkCodec {
    fn should_track(&self, path: &Path) -> bool {
        matches!(path.extension().and_then(|e| e.to_str()), Some("yaml" | "yml"))
    }
}
```

`CppHeaderWalkCodec` (tracks `.h` under `include/`, excludes `build/`/`generated/`) is
defined in `vast-noet`, not `noet-core` (application-neutral rule). Application shims
register additional codecs via `WALK_CODECS.register()` before `DocumentCompiler::new()`.

### `ClaimMap`

```noet-core/docs/project/ISSUE_68_TWO_REGISTRY_CODEC_ARCHITECTURE.md#L1-1
/// Parse-time registry mapping absolute file paths to the codec that claimed them.
///
/// Inner map value is Option<CodecFactory>: Some(factory) = claimed, None = rejected.
/// Populated during DocCodec::parse() of an owning (orchestrating) codec.
/// Read during parse_one_path codec selection, before CODECS.path_get.
pub struct ClaimMap(Arc<RwLock<HashMap<PathBuf, Option<CodecFactory>>>>);

impl ClaimMap {
    pub fn create() -> Self { ... }

    /// Claim a specific absolute path for a given codec factory.
    ///
    /// If the path is already claimed by a different factory, the new claim
    /// overwrites the old one and a tracing::warn! is emitted (claim conflict).
    pub fn claim(&self, abs_path: PathBuf, factory: CodecFactory) { ... }

    /// Reject a path — marks it as explicitly filtered (None sentinel).
    /// Rejected paths route to UnclaimedDataCodec + info, not process_asset.
    /// Added for Issue 72.
    pub fn reject(&self, abs_path: PathBuf) { ... }

    /// Look up the codec factory for an absolute path.
    /// Returns None if the path has not been claimed (or has been rejected).
    pub fn get(&self, abs_path: &Path) -> Option<CodecFactory> { ... }

    /// Returns true if the path has been explicitly rejected (None sentinel present).
    pub fn is_rejected(&self, abs_path: &Path) -> bool { ... }

    /// Remove a claim or rejection (used by on_file_deleted in the watch loop).
    pub fn unclaim(&self, abs_path: &Path) { ... }

    /// Number of currently registered entries (claims + rejections).
    pub fn len(&self) -> usize { ... }
}

pub static CLAIM_MAP: Lazy<ClaimMap> = Lazy::new(ClaimMap::create);
```

### Modified `net_dir_partition` filter

```noet-core/docs/project/ISSUE_68_TWO_REGISTRY_CODEC_ARCHITECTURE.md#L1-1
// Before:
if CODECS.get(&p_ap_file).is_some() {
    Some(p)
} else {
    None
}

// After:
if CODECS.get(&p_ap_file).is_some() || WALK_CODECS.should_track(&p) {
    Some(p)
} else {
    None
}
```

One line change, fully backward compatible. Existing Markdown files continue to pass
through `CODECS.get` (for `index.md`) or `WALK_CODECS.should_track` (for plain `.md` via
`MdWalkCodec`) as before.

### `UnclaimedDataCodec`

A minimal `DocCodec` implementation that produces no nodes and emits no relations. Used
as the codec for walk-tracked but unclaimed (or rejected) files. Prevents the compiler
from treating them as binary assets while cleanly communicating that no codec owns them.

```noet-core/docs/project/ISSUE_68_TWO_REGISTRY_CODEC_ARCHITECTURE.md#L1-1
/// No-op codec for walk-tracked files that no primary codec has claimed.
/// Emits a ParseDiagnostic::info and produces no BeliefBase nodes.
#[derive(Default, Clone)]
pub struct UnclaimedDataCodec;

impl DocCodec for UnclaimedDataCodec {
    fn proto(&self, _path: &Path) -> Result<Option<IRNode>, BuildonomyError> { Ok(None) }
    fn parse(&mut self, _content: &str, _current: IRNode,
             _proto_index: &ProtoIndex,
             _diagnostics: &mut Vec<ParseDiagnostic>) -> Result<(), BuildonomyError> { Ok(()) }
    fn nodes(&self) -> Vec<IRNode> { vec![] }
    fn inject_context(&mut self, _node: &IRNode, _ctx: &BeliefContext<'_>,
                      _diagnostics: &mut Vec<ParseDiagnostic>) -> Result<Option<BeliefNode>, BuildonomyError> { Ok(None) }
    fn finalize(&mut self, _diagnostics: &mut Vec<ParseDiagnostic>)
        -> Result<HashMap<Bid, IRNode>, BuildonomyError> { Ok(HashMap::new()) }
    fn generate_source(&self) -> Option<String> { None }
}
```

The `info`-level diagnostic (not `Warning`) was decided during Issue 72 discussion: unclaimed
tracked files are expected during normal operation when a walk codec tracks by extension but
claims have not yet fired.

### Ordering Guarantee

```noet-core/docs/project/ISSUE_68_TWO_REGISTRY_CODEC_ARCHITECTURE.md#L1-1
static init    WALK_CODECS populated (MdWalkCodec, YamlWalkCodec registered).
               Application shims (e.g. vast-noet) register additional codecs
               (e.g. CppHeaderWalkCodec) via WALK_CODECS.register() before
               DocumentCompiler::new().
               CLAIM_MAP empty.

ProtoIndex::build()
               WalkDir pass. net_dir_partition filter:
                 CODECS.get().is_some() || WALK_CODECS.should_track()
               → .yaml and .md files now appear in ProtoIndex child lists.
               → CLAIM_MAP still empty.

parse_sequential Phase 1 (all network index.md files, depth-grouped):
               NetworkCodec::parse() fires for each component index.md.
               → reads frontmatter, calls proto_index.children_of()
               → CLAIM_MAP.claim(path, SubCodecA factory)   ← written here
               → CLAIM_MAP.claim(path, SubCodecB factory)
               → CLAIM_MAP.reject(path)  ← for explicitly filtered children
               All network dirs at all depths complete. CLAIM_MAP now populated.

               Note: prepare_proto_relations fires later (from initialize_stack
               during Phase 2 ancestor-stack building). It builds Section edges
               on the IRNode only — it does NOT register CLAIM_MAP entries.

parse_sequential Phase 2 (leaf files, flat DFS):
               parse_one_path dispatches each file via four-branch lookup:
                 1. CLAIM_MAP.get()         → claimed codec
                 2. CLAIM_MAP.is_rejected() → UnclaimedDataCodec + info
                 3. CODECS.path_get()       → extension/stem registered codec
                 4. WALK_CODECS.should_track() → UnclaimedDataCodec + info
               No tracked file reaches process_asset.

parse_sequential Phase 3 (remainder: re-parses from unresolved refs)
               Claims already registered; CLAIM_MAP lookups succeed normally.
```

### Pre-flight `proto()` Workaround (Technical Debt)

`parse_one_path` in `DocumentCompiler` currently calls `proto()` on binary codecs
before invoking `parse_content`, routing `Ok(None)` results directly to
`process_asset` to avoid a loud WARN when (e.g.) an `.xlsx` file has no `index`
tab. This is a stopgap that duplicates the "is this file a real document?" check
that should be the responsibility of the two-registry dispatch layer.

**Correct fix**: The two-registry architecture should express this natively via a
first-class "proto() returned None → asset" path in the unified dispatch. Once
stabilised, the pre-flight block in `parse_one_path` (`compiler.rs`) should be
removed. See: `src/codec/compiler.rs` — search for `Pre-flight proto() check`.

### Unclaimed Tracked Files

If `WALK_CODECS.should_track()` returns true but `CLAIM_MAP` has no entry for a file,
`parse_one_path` routes it to `UnclaimedDataCodec`. This codec:

- Emits a `ParseDiagnostic::info` naming the file
- Produces no `IRNode`s and no `BeliefBase` nodes
- Does **not** reach `process_asset` — the file is correctly identified as structured text,
  just not owned by anyone

This is the correct fallback: the file is visible, the info message is actionable, and the
build continues.

### Watch Loop and Re-parse Correctness

When a file is deleted, `DocumentCompiler::on_file_deleted` must call `CLAIM_MAP.unclaim(path)`.

When an orchestrating document changes, `on_file_modified` re-parses its codec's `parse()`.
The codec re-runs and re-claims the correct child paths. `CLAIM_MAP.claim()` uses overwrite
semantics — stale claims from the previous parse are replaced automatically.

**`parse_epoch` (parallel re-parse)**: Claims registered during the initial
`parse_sequential` Phase 1 are present in `CLAIM_MAP` for all subsequent `parse_epoch`
calls. Re-parse of a network index re-registers claims before its children are re-queued,
because the remainder queue sorts by parse count then path order — the network index always
has a lower count than its children and is therefore processed first within any epoch batch.

## Deviations from Original Spec

| Item | Original | Actual |
|------|----------|--------|
| `DocCodec::parse` signature | Unchanged | Gained `proto_index: &ProtoIndex` parameter |
| Claiming call site | `prepare_proto_relations` | `NetworkCodec::parse()` — fires in Phase 1 |
| `CODECS` bare `.md` entry | Retained | Removed — `.md` dispatch via `CLAIM_MAP` / `MdWalkCodec` only |
| `ClaimMap` inner type | `HashMap<PathBuf, CodecFactory>` | `HashMap<PathBuf, Option<CodecFactory>>` (reject sentinel for Issue 72) |
| `CppHeaderWalkCodec` location | `noet-core` | `vast-noet` (application-neutral rule) |
| Unclaimed file diagnostic | `ParseDiagnostic::Warning` | `ParseDiagnostic::info` |
| `MdWalkCodec` | Not in spec | Added — `.md` files tracked via `WALK_CODECS` |

## Implementation Steps

### Phase 1: Core infrastructure in `src/codec/mod.rs` (1.5 days)

1. **`WalkCodec` trait and `WalkCodecMap`** (0.5 day)
   - [x] Define `WalkCodec` trait with `should_track(&self, path: &Path) -> bool`
   - [x] Implement `WalkCodecMap`: `Arc<RwLock<Vec<Box<dyn WalkCodec>>>>`, `register()`, `should_track()`
   - [x] Add `pub static WALK_CODECS: Lazy<WalkCodecMap>`
   - [x] Implement `YamlWalkCodec` (`.yaml` / `.yml` by extension)
   - [x] Implement `MdWalkCodec` (`.md` / `.markdown` by extension) — **added; not in original spec**
   - [x] Register `MdWalkCodec` and `YamlWalkCodec` in `WALK_CODECS` at startup
   - [x] `CppHeaderWalkCodec` implemented in `vast-noet`, not `noet-core` — **moved; application-neutral rule**

2. **`ClaimMap`** (0.5 day)
   - [x] Implement `ClaimMap`: `Arc<RwLock<HashMap<PathBuf, Option<CodecFactory>>>>`, `claim()`, `reject()`, `is_rejected()`, `get()`, `unclaim()`, `len()`
   - [x] Add `pub static CLAIM_MAP: Lazy<ClaimMap>`
   - [x] `claim()` emits `tracing::warn!` on overwrite (conflict detection)
   - [x] Inner type is `Option<CodecFactory>` — `None` sentinel used for `reject()` — **deviation from original spec**

3. **`UnclaimedDataCodec`** (0.25 day)
   - [x] Implement all `DocCodec` methods as no-ops
   - [x] Emits `ParseDiagnostic::info` (not `Warning`) — **decided during Issue 72**

4. **Unit tests for `WalkCodecMap` and `ClaimMap`** (0.25 day)
   - [x] `should_track`: `.yaml` → true, `.md` → true (MdWalkCodec), `.h` → false (not in noet-core WALK_CODECS)
   - [x] `claim` + `get` round-trip; `unclaim` removes entry; conflict emits tracing warn
   - [x] `ClaimMap::len()` reflects claim count accurately

### Phase 2: Wire into `ProtoIndex` and `DocumentCompiler` (1.5 days)

5. **`net_dir_partition` filter** — `src/codec/proto_index.rs` (0.25 day)
   - [x] Add `|| WALK_CODECS.should_track(&p)` to the file inclusion filter
   - [x] Unit test: fixture directory with a `.yaml` file — assert it appears in `children_of`
   - [x] Unit test: existing `.md` tests still pass (no regression)

6. **`parse_one_path` codec selection** — `src/codec/compiler.rs` (0.5 day)
   - [x] Add `CLAIM_MAP.get(&file_path)` pre-check before `CODECS.path_get`
   - [x] Add `CLAIM_MAP.is_rejected()` branch for explicit rejection sentinel
   - [x] Add `WALK_CODECS.should_track` branch for unclaimed-tracked case → `UnclaimedDataCodec`
   - [x] Diagnostic level is `ParseDiagnostic::info` not `Warning` — **decided during Issue 72**

7. **Watch loop: `on_file_deleted` clears claim** — `src/codec/compiler.rs` (0.25 day)
   - [x] `on_file_deleted` calls `CLAIM_MAP.unclaim(path)`

8. **Integration test: end-to-end ordering guarantee** (0.5 day)
   - [ ] Fixture: `index.md` with a codec that claims a sibling `.yaml` file during `parse()`; assert `.yaml` dispatches to claimed codec, not `UnclaimedDataCodec` — **tracked on Issue 67 Phase 2 step 9**

### Phase 3: Documentation and exports (1 day)

9. **Public API exports** (0.25 day)
   - [x] Export `WalkCodec`, `WalkCodecMap`, `WALK_CODECS` from `src/codec/mod.rs`
   - [x] Export `ClaimMap`, `CLAIM_MAP` from `src/codec/mod.rs`
   - [x] Export `UnclaimedDataCodec` from `src/codec/mod.rs`

10. **WASM stub** (0.25 day)
    - [x] `WALK_CODECS` and `CLAIM_MAP` are `#[cfg(not(target_arch = "wasm32"))]` gated — structural correctness confirmed
    - [ ] WASM viewer extension classification gap documented in `.scratchpad/wasm_codec_registry_gap.md`; full fix (codec manifest) tracked there as a future issue

11. **Docstrings** (0.5 day)
    - [x] Full rustdoc on `WalkCodec`, `WalkCodecMap`, `WALK_CODECS`, `ClaimMap`, `CLAIM_MAP`, `UnclaimedDataCodec` in `src/codec/mod.rs`
    - [ ] AGENTS.md / CONTRIBUTING.md claiming-pattern note — deferred

12. **Design doc updates** (0.25 day)
    - [x] `docs/design/beliefbase_architecture.md` §3.0 and §3.2 updated: `Source Files` line extended to include `.yaml`; WALK_CODECS/CLAIM_MAP callout block replaced with current description; new §3.2 subsection "Two-Registry Codec Dispatch" added
    - [ ] `docs/design/architecture.md` §11 — deferred

## Testing Requirements

### Unit Tests

- `WalkCodecMap::should_track`: positive and negative cases for `MdWalkCodec` and `YamlWalkCodec`
- `ClaimMap`: claim/get/unclaim cycle; reject/is_rejected cycle; overwrite conflict warning; concurrent read/write safety
- `net_dir_partition` with `.yaml` files present: asserts they appear in child lists
- `parse_one_path` unclaimed-tracked path: `ParseDiagnostic::info`, no `BuildonomyError`
- `parse_one_path` claimed path: correct codec dispatched

### Integration Tests

- Full `parse_sequential` with a fixture claiming codec + sibling `.yaml` file: YAML parsed by claimed codec, node appears in `BeliefBase` — **deferred to Issue 67 Phase 2**
- Existing full test suite passes with no regressions

### Regression Guard

- `CODECS.path_get` path for `index.md` files unchanged
- `ClaimMap` returns `None` for paths not explicitly claimed — no accidental claim of existing codec files
- `CodecMap::get` extension-only fallback bug fixed: stem-constrained entries no longer match on extension alone

## Success Criteria

- [x] `.yaml` files appear in `ProtoIndex` child lists for any network containing them, with no build step required.
- [x] An owning codec can call `CLAIM_MAP.claim(path, factory)` during `parse()` and the claimed file is subsequently dispatched to the correct codec in the same `parse_sequential` run.
- [x] Tracked-but-unclaimed files produce `ParseDiagnostic::info`, not `BuildonomyError`, and produce no `BeliefBase` nodes.
- [x] All existing tests pass.
- [ ] WASM build passes — not verified this session.
- [ ] `cargo build` succeeds without system-level Clang, libclang, or CMake — not verified (no C++ codec in `noet-core`; `CppHeaderWalkCodec` moved to `vast-noet`).

## Risks

- **Risk: `CLAIM_MAP` global state between test runs** — Tests that register claims must
  clean up after themselves, or use a scoped `ClaimMap` instance. → **Mitigation**: Provide
  a `ClaimMap::new()` constructor for test use; integration tests that touch `CLAIM_MAP` run
  serially or use a local instance passed via `DocumentCompiler::with_claim_map()`. Full
  isolation requires claiming to be injectable through `parse()` — deferred.

- **Risk: Ordering violation in parallel `parse_epoch`** — `parse_epoch` spawns tasks
  concurrently; a YAML file could be dispatched before its owning `index.md` has registered
  its claim in the same epoch batch. → **Mitigation**: `parse_epoch` is not used for the
  initial Phase 1/Phase 2 split in `parse_sequential` — those phases are strictly sequential.
  By the time any `parse_epoch` re-parse batch runs, Phase 1 has already completed and all
  claims are registered. The remaining edge case — an orchestrating index and its YAML
  children both modified and re-queued into the same `parse_epoch` batch — is handled by
  remainder-queue sort order: the `index.md` has a lower parse count than its YAML children
  and therefore precedes them within the batch.

- **Risk: Memory growth from stale claims in long-running watch sessions** — Claims
  accumulate over many file modification cycles if `unclaim` is not called. →
  **Mitigation**: `on_file_deleted` should call `unclaim` (not yet implemented — step 7).
  A periodic `CLAIM_MAP.gc()` method can be added if needed; defer until observed in practice.

## Open Questions

1. **`ClaimMap` as parameter vs. global** — ~~Recommend global `CLAIM_MAP` for consistency.~~
   **Resolved**: global `CLAIM_MAP` used. `DocumentCompiler::with_claim_map()` exists for
   test isolation. Full per-parse isolation would require `ClaimMap` to be injectable through
   `parse()` — deferred as a follow-on.

2. **`WALK_CODECS` registration timing** — ~~Recommend registering in the `Lazy` static
   initializer.~~
   **Resolved**: `MdWalkCodec` and `YamlWalkCodec` registered in the static initializer.
   Application shims (e.g. `vast-noet`) register additional codecs (e.g. `CppHeaderWalkCodec`)
   via `WALK_CODECS.register()` before `DocumentCompiler::new()`.

3. **`CppHeaderWalkCodec` / `CppHeaderCodec` location** — ~~Should they use `CODECS` or
   `CLAIM_MAP`? In `noet-core` or application shim?~~
   **Resolved**: moved to `vast-noet` entirely (application-neutral rule). `noet-core`
   defines only `MdWalkCodec` and `YamlWalkCodec`.

4. **WASM stubs** — ~~Need stub implementations for `WALK_CODECS` and `CLAIM_MAP`.~~
   **Resolved structurally**: `WALK_CODECS` and `CLAIM_MAP` are `#[cfg(not(target_arch = "wasm32"))]`
   gated. WASM codec registry gap documented as a future issue; WASM build not verified this
   session.

## References

- Issue 67: Horizon FSW Source Codec — primary consumer; Phase 2 (YAML sub-codecs) is blocked on this issue
- Issue 72: Rejection sentinel (`ClaimMap::reject` / `is_rejected`) added during that work
- `src/codec/proto_index.rs` — `net_dir_partition()`: the `|| WALK_CODECS.should_track` change
- `src/codec/compiler.rs` — `parse_one_path()`: four-branch codec selection
- `src/codec/mod.rs` — `CodecMap`, `CodecFactory`, `CODECS` — reference implementations
- `src/codec/builder.rs` — `AssetCodec` — reference for `UnclaimedDataCodec` minimal implementation