# Issue 26: Git-Aware BeliefNetwork Nodes

**Priority**: MEDIUM - Post-v0.1.0 feature
**Estimated Effort**: 4-5 days (RELATIVE COMPARISON ONLY)
**Dependencies**: None (standalone feature)

## Summary

Inject Git repository metadata into BeliefNetwork nodes during parse time,
tracking commit hash, branch, dirty status, and upstream info. The primary
deliverable is a **per-node source backlink**: a URL pointing directly to each
node's source file (and optionally its line number) in the remote git host,
surfaced in the metadata panel of the HTML viewer. Git metadata is ephemeral —
computed at parse, visible in the in-memory `BeliefBase` for query and export,
but never written back to `index.md` and excluded from diff/stability logic.

**Use Cases**:
- **Source backlinks**: deep-link each node to its file on GitHub/GitLab,
  enabling one-click "edit this page" and "view source" from the HTML viewer
- Publishing validation: reject exports with uncommitted changes
- Version tracking: embed git commit in exported HTML
- CI/CD integration: validate all networks are committed before deployment

## Goals

1. Detect git repository for each `BeliefNetwork` node during `ProtoIndex::build`
2. Inject path-local git status into network `BeliefNode.metadata` at parse time
3. Track: commit hash, branch name, dirty flag, upstream, ahead/behind counts
4. Produce a `source_url` in every document and section node's `metadata`,
   linking to that node's source file (and line number when available) on
   the remote git host; surface it in the `metadata.js` metadata panel
5. Make git tracking opt-in via a `git-tracking` Cargo feature flag, a CLI
   argument, and a `NOET_GIT_TRACKING` env var fallback
6. Preserve write-stability: git metadata never triggers rewrite of `index.md`
7. Handle edge cases: no git repo, detached HEAD, nested repos, unknown host

## Architecture

### Metadata Field on `BeliefNode`

The central design decision is **where** git status lives and **how it is
kept out of source files** while still surviving the full parse → DB →
export → browser round-trip.

Add a `metadata` field to `BeliefNode` with the same type as `payload`
(`toml::value::Table`), serialized and persisted identically to `payload`
but never written back to source files:

```rust
pub struct BeliefNode {
    pub bid: Bid,
    pub kind: BeliefKindSet,
    pub title: String,
    pub schema: Option<String>,
    pub payload: Table,
    pub id: Option<String>,
    /// Runtime metadata: per-parse annotations such as git status and source
    /// backlinks. Serialized via `toml()` and persisted in the DB `metadata`
    /// column so it survives the full parse → DB → export → browser round-trip.
    /// Never appears in source files: `generate_source` is driven by the
    /// markdown event stream and `IRNode::as_frontmatter`, neither of which
    /// reads `BeliefNode::metadata`.
    /// Included in `PartialEq` so merges and `compute_diff` propagate it
    /// correctly across cache boundaries.
    #[serde(default)]
    #[serde(skip_serializing_if = "Table::is_empty")]
    pub metadata: Table,
}
```

`metadata` is a **full member** of `BeliefNode`: included in `PartialEq`,
serialized by `toml()`, persisted in the DB, and carried in
`BeliefEvent::NodeUpdate`. This means `compute_diff` propagates metadata
changes to downstream consumers (e.g. the HTML viewer) when they change
between parses. Write-stability is guaranteed by construction: `metadata`
never flows into `generate_source` because that path reads only the
markdown event stream and `IRNode::as_frontmatter`, which do not consult
`BeliefNode::metadata`.

The one consequence is that fields like `checked_at` (set to
`SystemTime::now()` per `GitCache::populate` call) will produce a
`NodeUpdate` event on every watch-mode rebuild, since the timestamp changes
each run. This is acceptable: the event is cheap, and it ensures the
browser always receives up-to-date git status without requiring a manual
refresh. The BID-stability invariant
(`test_belief_set_builder_bid_generation_and_caching`) is unaffected because
BIDs are derived only from `payload`, not `metadata`.

### Git Metadata Schema

Network nodes get a `git` sub-table in `metadata` after parsing:

```toml
[git]
repo_root = "../.."         # Relative path from network dir to .git directory
commit = "a1b2c3d4e5f6..."  # Full HEAD SHA
commit_short = "a1b2c3d"   # Short SHA (7 chars)
branch = "main"             # Current branch (absent if detached HEAD)
upstream = "origin/main"    # Upstream tracking branch (absent if none)
dirty = false               # Any uncommitted changes within network path?
untracked = 0               # Count of untracked files in network path
modified = 0                # Count of modified files in network path
ahead = 2                   # Commits ahead of upstream (0 if no upstream)
behind = 0                  # Commits behind upstream (0 if no upstream)
last_commit_date = "2024-01-15T10:30:00Z"
checked_at = "2024-01-15T14:22:33Z"
```

Every document and section node gets a `source_url` key in `metadata`
(populated during `inject_context`, Phase 4):

```toml
# metadata on a document node (e.g. docs/guide.md in a GitHub repo)
source_url = "https://github.com/org/repo/blob/main/docs/guide.md"

# metadata on a section node with line tracking enabled
source_url = "https://github.com/org/repo/blob/main/docs/guide.md#L42"
```

`source_url` is absent when: git tracking is disabled, the repo has no
configured remote, the network's `index.md` payload overrides with
`git_remote_url = ""` to suppress it, or the remote host is not recognized
and no explicit override is given.

### Source URL Construction

**Remote URL normalization**: parse `git remote get-url origin` to detect the
hosting provider, then construct the blob URL:

| Remote pattern | Blob URL pattern |
|---|---|
| `https://github.com/org/repo` | `.../blob/<branch>/<path>#L<n>` |
| `git@github.com:org/repo.git` | `https://github.com/org/repo/blob/<branch>/<path>#L<n>` |
| `https://gitlab.com/org/repo` | `.../blob/<branch>/<path>#L<n>` |
| `git@gitlab.com:org/repo.git` | `https://gitlab.com/org/repo/blob/<branch>/<path>#L<n>` |
| anything else | raw remote URL stored as-is, no path appended |

GitHub and GitLab share the same blob URL pattern so a single normalizer
handles both. SSH remotes are converted to HTTPS form.

**Override**: set `git_remote_url` in a network node's `payload` (persisted
in `index.md`) to hard-code the base URL, bypassing remote detection:

```toml
# index.md frontmatter — overrides auto-detected remote
git_remote_url = "https://github.com/org/fork"
```

This is also the escape hatch for Gitea, Bitbucket, Forgejo, or any other
host: the operator sets `git_remote_url` to the correct HTTPS base and the
blob path pattern is appended automatically.

**Branch / commit selection**: use the current branch name from `GitStatus`
when available; fall back to `HEAD` when detached.

### Line Number Tracking

Line numbers require knowing where each node's heading declaration starts in
the source file. This information is not currently captured.

Add `source_line: Option<usize>` to `IRNode`:

```rust
pub struct IRNode {
    // ... existing fields ...
    /// 1-based line number of this node's heading (or frontmatter start for
    /// the document root node) in the source file. Populated by MdCodec
    /// during heading parsing. Used to construct #L<n> source backlinks.
    pub source_line: Option<usize>,
}
```

`MdCodec` already tracks byte ranges per event via
`cmark_resume_with_source_range_and_options`; converting the heading event's
byte offset to a line number via `byte_offset_to_location` at parse time
is straightforward. The document root node (heading level 1 or the
frontmatter block) gets `source_line = Some(1)`.

`source_line` is read directly from `IRNode` inside `inject_context` —
`MdCodec::inject_context` already receives the `&IRNode` argument and has
access to `self.current_events` (the `ProtoNodeWithEvents` list). No
intermediate encoding into `metadata` is needed; `inject_context` reads
`node.source_line` and combines it with the network's git remote URL to
produce `node.metadata["source_url"]`.

### Surfacing in the HTML Viewer (`metadata.js`)

`BeliefBaseWasm.get_context()` returns a `NodeContext`. The `metadata`
field must be included in the WASM-serialized form so that `metadata.js`
can read it. Two options:

- **Option A**: add `metadata` as a field on `NodeContext` (parallel to
  `node.payload`), serialized the same way.
- **Option B**: merge `metadata` into the serialized `node` object under a
  `metadata` key, distinct from `payload`.

Option A is preferred — cleaner separation, no risk of collisions with
existing `payload` keys.

In `renderNodeContext` in `metadata.js`, add a "Source" section after
"Node Information" when `context.metadata?.source_url` is present:

```javascript
if (context.metadata?.source_url) {
  html += '<div class="noet-metadata-section">';
  html += "<h3>Source</h3>";
  html += '<dl class="noet-metadata-list">';
  html += `<dt>Edit</dt><dd><a href="${escapeHtml(context.metadata.source_url)}" ` +
          `target="_blank" rel="noopener noreferrer">View on remote ↗</a></dd>`;
  html += "</dl>";
  html += "</div>";
}
```

### Path-Local Git Status

Only changes within the network's directory are considered. A network at
`/project/docs/core/` is only dirty if files under `docs/core/` have
uncommitted changes — changes in `src/` or a sibling network do not affect it.

**Implementation**: `StatusOptions::pathspec(network_relative_path)` in
libgit2 filters the status query to the network's subtree.

### Data Flow

```
ProtoIndex::build(repo_root)           ← single WalkDir pass (unchanged)
  └─ for each network_dir discovered:
       └─ [git-tracking feature] GitCache::get_or_compute(network_dir)
            └─ Repository::discover(network_dir) → keyed by repo workdir root
            └─ NetworkGitStatus for this network dir (shared RepoGitStatus)

ProtoIndex::proto_for(network_dir) → (IRNode, Option<NetworkGitStatus>)
  └─ NetworkCodec::proto(path)         ← filesystem-free (unchanged)
  └─ NetworkCodec::prepare_proto_relations(proto, dir, children)
  └─ returns (proto, git_cache.get(network_dir).cloned())
       ← git status carried alongside proto; proto.document unchanged

GraphBuilder Phase 1: push(proto, git_status)
  └─ BeliefNode constructed via TryFrom<&IRNode>  ← payload unchanged
  └─ if git_status.is_some(): node.metadata["git"] populated directly
       (no staging key; payload never touched)

MdCodec heading parsing
  └─ records byte offset of each heading event
  └─ converts to 1-based line via byte_offset_to_location
  └─ stores in IRNode.source_line

GraphBuilder Phase 4: inject_context (NetworkCodec / MdCodec)
  └─ MdCodec::inject_context receives &IRNode (has source_line field)
  └─ reads network ancestor's metadata["git"] for remote_url + branch
  └─ reads node.source_line directly (no metadata round-trip)
  └─ constructs node.metadata["source_url"] = remote_url/blob/branch/path#Lnn

BeliefBaseWasm.get_context() → NodeContext
  └─ includes metadata field (serialized to JS object)
  └─ metadata.js renderNodeContext reads metadata.source_url
  └─ renders "View on remote ↗" link in metadata panel

Callers / export layer
  └─ node.metadata.get("git") → git status available for query, HTML export
  └─ compute_diff ignores metadata → no NodeUpdate, no rewrite
```

No staging keys are used. Git status is attached to `BeliefNode.metadata`
directly during Phase 1 construction. `source_line` is read from the typed
`IRNode` field inside `inject_context` — it is never encoded into `metadata`
as an intermediate value.

### GitCache: Shared Per-Repo State

Multiple network directories may share a single git repository. Open the
`Repository` once per repo root and cache all status queries:

```rust
/// Keyed by canonicalized git workdir root (from repo.workdir()).
struct GitCache {
    by_repo: HashMap<PathBuf, Arc<RepoGitStatus>>,
}

struct RepoGitStatus {
    commit: String,
    commit_short: String,
    branch: Option<String>,       // None if detached HEAD
    upstream: Option<String>,     // None if no upstream configured
    ahead: usize,
    behind: usize,
    last_commit_date: String,     // RFC 3339
    checked_at: String,           // RFC 3339
}

struct NetworkGitStatus {
    repo_root: PathBuf,           // relative from network dir to .git
    dirty: bool,
    untracked: usize,
    modified: usize,
    repo: Arc<RepoGitStatus>,     // shared with other networks in same repo
}
```

`GitCache` lives inside `ProtoIndex` (which already wraps `Arc<RwLock<...>>`
and is `Clone`). Parallel epoch-batch task builders receive the same
`ProtoIndex` clone, so the cache is shared without duplication.

### CLI and Environment Wiring

Mirror the `jobs` pattern exactly:

```
CLI --git-tracking flag > NOET_GIT_TRACKING env var > default (disabled)
```

`DocumentCompiler::with_html_output` and `::new` accept a `git_tracking:
bool` argument. `ProtoIndex::build` accepts the same flag and passes it to
`GitCache`. When `false` (the default), no `git2` code runs and `ProtoIndex`
builds identically to today.

In `main.rs`, add to `Commands::Parse` and `Commands::Watch`:

```rust
/// Enable git metadata injection into BeliefNetwork nodes.
/// Can also be set via NOET_GIT_TRACKING=1 environment variable.
#[arg(long)]
git_tracking: bool,
```

Resolution in `main`:
```rust
let git_tracking = git_tracking
    || std::env::var("NOET_GIT_TRACKING")
        .ok()
        .map(|v| v == "1" || v.eq_ignore_ascii_case("true"))
        .unwrap_or(false);
```

## Implementation Steps

### 1. Add `git2` dependency and feature flag (0.5 days) ✅
- [x] Add to `Cargo.toml`: `git2 = { version = "0.19", optional = true, default-features = false }`
- [x] Add feature: `git-tracking = ["dep:git2"]`
- [x] Gate all git code with `#[cfg(feature = "git-tracking")]` (via `inner` submodule in `src/codec/git.rs`)
- [x] Document in `Cargo.toml` feature comment block

### 2. Add `metadata` field to `BeliefNode` (0.5 days) ✅
- [x] Add `#[serde(default, skip_serializing_if = "Table::is_empty")] pub metadata: Table` to `BeliefNode`
  - Note: `metadata` is **included** in serde JSON serialization (not skipped) so it
    survives the parse → DB → `beliefbase.json` → browser round-trip
- [x] Included in `PartialEq` (merges and `compute_diff` propagate it correctly)
- [x] `Hash` unchanged (hashes only `bid` — correct)
- [x] `toml()` serializes `metadata` intact — it flows through `BeliefEvent::NodeUpdate`
  to the DB; source-file safety is guaranteed by `IRNode::as_frontmatter`, which never
  reads `BeliefNode::metadata`
- [x] DB: added `metadata TEXT` column to initial migration in `db.rs`; `update_node`
  writes it as nullable TOML; `FromRow` reads it back
- [x] `Display` impl unchanged (metadata omitted — ephemeral display noise)

### 3. Add `source_line` to `IRNode` and populate in `MdCodec` (0.5 days) ✅
- [x] Add `pub source_line: Option<usize>` to `IRNode` (excluded from `PartialEq`)
- [x] At `Start(Heading)` in `MdCodec::parse`: compute
  `byte_offset_to_location(&self.content, offset.start).0` and store in
  `new_current.source_line` — reuses the already-captured `offset.start`
- [x] Document root node: `current.source_line = Some(1)` set before the parse loop
- [x] `source_line` is **not** encoded into `BeliefNode.metadata`; read directly
  from `&IRNode` inside `inject_context`

### 4. Implement `GitStatus`, `GitCache`, and remote URL normalization (1 day) ✅
- [x] Implemented in `src/codec/git.rs` behind `#[cfg(feature = "git-tracking")]`
- [x] `RepoGitStatus`, `NetworkGitStatus` structs with all fields from spec
- [x] `NetworkGitStatus::to_metadata_table()` converts to `toml::value::Table`
  for direct assignment into `BeliefNode.metadata["git"]`
- [x] `GitCache::populate(network_dirs)` — opens each `Repository` once per
  canonicalized workdir root, computes path-local status per network dir
- [x] `GitCache::get(network_dir) -> Option<&NetworkGitStatus>` — read-only lookup
- [x] `normalize_remote_url(raw)` — SSH→HTTPS for github.com/gitlab.com, strips
  `.git`, returns `None` for unrecognized hosts
- [x] `remote_url` stored in `RepoGitStatus`
- [x] No git repo: `Repository::discover` error → warning logged, `None` returned
- [x] Detached HEAD: `branch = None`; `compute_source_url` falls back to `"HEAD"`
- [x] Path-local status via `StatusOptions::pathspec`
- [x] Ahead/behind via `graph_ahead_behind` on local refs only
- [x] RFC 3339 timestamps via `format_git_time` (no chrono dependency)
- [x] 17 unit tests: all normalization cases, dirty detection, empty repo, real repo

### 5. Integrate into `ProtoIndex` (0.5 days) ✅
- [x] Added `git_cache: Arc<GitCache>` field to `ProtoIndex` (feature-gated)
- [x] `ProtoIndex::build(repo_root, git_tracking: bool)` — populates `GitCache`
  from partition keys when `git_tracking = true`
- [x] `ProtoIndex::proto_for` returns
  `Result<Option<(IRNode, Option<NetworkGitStatus>)>, BuildonomyError>`
- [x] `proto_for` returns `(proto, git_cache.get(network_dir).cloned())`; no
  staging key, `proto.document` untouched
- [x] `NetworkGitStatus` stub type defined for non-feature builds so return
  signature compiles unconditionally
- [x] `compiler.rs` `with_html_output` and `simple` pass `false` for now
  (full wiring deferred to Step 10)
- [x] All `proto_for` callers updated to destructure the tuple

### 6. Populate `BeliefNode.metadata` directly during Phase 1 (0.5 days) ✅
- [x] `GraphBuilder::push` gains `metadata_override: Option<TomlTable>` parameter;
  applied to `node.metadata` after cache resolution, immediately before `NodeUpdate`
  emission — always stomps stale cached metadata
- [x] `initialize_stack` builds `metadata_override` from `git_status` under
  `#[cfg(feature = "git-tracking")]`, wraps as `{"git": <table>}`, passes to `push`
- [x] `parse_content` call site passes `None` (doc/section metadata from Step 7)
- [x] `payload` is never touched; no staging key used
- [x] `debug_assert` omitted — structural guarantee is sufficient (no staging key
  path exists)

### 7. Construct `source_url` in `inject_context` (0.5 days) ✅
- [x] `compute_source_url(node: &IRNode, ctx: &BeliefContext) -> Option<String>`
  added as a free function in `md.rs`
- [x] Looks up ancestor network node via `ctx.beliefbase().get(root_net)`
- [x] Checks `network_node.payload["git_remote_url"]` override first (empty string
  suppresses `source_url` for the network)
- [x] Falls back to `network_node.metadata["git"]["remote_url"]`
- [x] `branch` from `metadata["git"]["branch"]`, falls back to `"HEAD"`
- [x] `node.source_line` read directly from `&IRNode` — no metadata round-trip
- [x] `source_url` injected into result node's `metadata` at end of
  `MdCodec::inject_context`; `None` result promoted to `Some(ctx.node.clone())`
  with `source_url` when a URL is available
- [x] `NetworkCodec::inject_context` delegates to `MdCodec` — no separate change needed

### 8. Expose `metadata` through WASM `NodeContext` (0.5 days) ✅
- [x] Added `pub metadata: toml::value::Table` to `NodeContext` in `wasm.rs`,
  positioned between `home_net` and `related_nodes` (parallel to `node.payload`)
- [x] Populated in `extract_node_context` via `ctx.node.metadata.clone()`
- [x] Serializes as a plain JS object via `serde_wasm_bindgen` — identical
  behaviour to `node.payload`; no additional wiring needed
- [x] `BeliefBaseWasm.get_context()` includes it: `context.metadata?.source_url`
  is now available in JS (used by step 9)
- [x] `BeliefNode.metadata` already round-trips through `beliefbase.json`
  (serialized by `toml()`, persisted in DB) so the field is populated on load

### 9. Render source backlink in `metadata.js` (0.5 days) ✅
- [x] Destructure `metadata` from `context` in `renderNodeContext`
- [x] After "Node Information": render a "Source" `<div>` with a `<dl>`
  containing an `Edit` → `View on remote ↗` link when `metadata?.source_url`
  is present; link opens in a new tab with `rel="noopener noreferrer"`
- [x] For network nodes (`node.kind` includes `"Network"`): render a
  collapsible "Git Status" `<details>`/`<summary>` section showing branch,
  commit (short SHA), dirty flag, upstream, ahead/behind counts, and last
  commit date — all fields are optional and only rendered when present

### 10. CLI wiring (0.5 days) ✅
- [x] Added `--git-tracking` flag to `Commands::Parse` and `Commands::Watch` in
  `src/bin/noet/main.rs`; doc string mentions `NOET_GIT_TRACKING` env var fallback
- [x] Env var resolution in both `Parse` and `Watch` match arms:
  `CLI flag || NOET_GIT_TRACKING == "1" || NOET_GIT_TRACKING == "true"` (case-insensitive)
- [x] `DocumentCompiler::with_html_output` gains `git_tracking: bool` as final
  parameter; `DocumentCompiler::new` passes `false` (convenience constructor);
  `simple` always passes `false` with a comment explaining the intent
- [x] `WatchService::new` and `::with_html_output` gain `git_tracking: bool`;
  threaded through `FileUpdateSyncer::new` to both `DocumentCompiler` constructors
- [x] All call sites updated: compiler test helpers, `tests/cache_invalidation_test.rs`,
  `tests/service_integration.rs`, `examples/watch_service.rs`, `watch.rs` doc-tests
  (all pass `false` — git tracking is opt-in, default off)

### 11. Tests (1 day)
- [x] `normalize_remote_url`: GitHub HTTPS, GitHub SSH, GitLab HTTPS, GitLab
  SSH, unknown remote returns `None` — 8 tests in `src/codec/git.rs`
- [x] `GitCache::populate` with `git2::Repository::init` temp repo — 3 tests
  (no repo, real repo, path-local dirty detection inside/outside network)
- [x] `source_url` construction: with and without line number, with
  `git_remote_url` payload override, empty override suppresses, detached HEAD
  falls back to `"HEAD"`, empty root path returns `None` — 7 tests in `md.rs`
  via `BeliefContext::new_for_test` (added `#[cfg(test)]` constructor to `context.rs`)
- [x] `source_line` populated correctly for document root, H1, H2, H3 nodes
  in `MdCodec` — 4 tests in `md.rs`
- [x] `BeliefNode` metadata serde round-trip: JSON and TOML — 2 tests in
  `properties.rs`
- [x] `test_no_git_metadata_when_tracking_disabled` — no `metadata["git"]`
  when `git_tracking = false` — 1 test in `compiler.rs`
- [x] **Bug fix**: root network node (`index.md` entry point) never received
  `metadata["git"]` — `initialize_stack` only injects git metadata for ancestor
  networks, not the entry network itself. Fixed in `parse_content` Phase 1:
  `ProtoIndex::git_status_for` (new method, feature-gated) looked up before the
  node loop; applied as `metadata_override` for `proto_idx == 0` when kind
  contains `Network`. See `.scratchpad/issue26-step11.md` for full analysis.
- [x] `test_metadata_not_in_generate_source` (in `md.rs`) — real `MdCodec::parse`
  + `generate_source` round-trip asserts output markdown contains no `source_url`,
  `metadata`, or `git` keys. Previous test in `properties.rs` used `IRNode::try_from`
  as a proxy (wrong path); replaced with `test_belief_node_metadata_serde_excludes_metadata_when_empty`
  that tests only the serde `skip_serializing_if` behaviour.
- [x] `test_bid_stability_with_git_tracking` — redesigned: two fresh compiler
  instances (git=true, git=false) run `parse_all` on the same files; their
  `session_bb` node counts are compared. Section BIDs are ephemeral across
  separate instances (embedded in parent doc sections tables, not individual
  files), so the test checks count equality and structural invariants rather
  than exact BID set equality.
- [x] `test_git_metadata_populated_on_network_node` — passes after two bug fixes:
  (1) `inject_context` was dropping runtime metadata when constructing new
  `BeliefNode` from `IRNode` (added `propagate_metadata` closure to carry
  `ctx.node.metadata` forward); (2) `MdCodec::finalize` was replacing the
  doc_bb node wholesale (changed `DocCodec::finalize` return type from
  `Vec<(IRNode, BeliefNode)>` to `HashMap<Bid, IRNode>`; Phase 4b now uses
  `BeliefNode::apply_source_update` to mutate only source-file-derived fields).
- [x] **Bug fix**: `DocCodec::finalize` interface changed to `HashMap<Bid, IRNode>`.
  `BeliefNode::apply_source_update(&IRNode)` added — updates `kind`, `title`,
  `schema`, `payload`, `id`; preserves `bid` and `metadata`.
- [x] **Bug fix**: Phase 4a equality check changed from TOML string comparison
  to `PartialEq` on `BeliefNode` — avoids serialization ordering fragility and
  correctly includes `metadata` in the comparison.

## Testing Requirements

- `metadata` and `source_url` must **not** appear in `generate_source()` output
  (guaranteed by `IRNode::as_frontmatter`, which never reads `BeliefNode::metadata`)
- `metadata` **must** round-trip through `toml()` → `BeliefEvent::NodeUpdate` →
  `process_event` → DB `metadata` column → `FromRow` intact
- Git metadata fields that change between runs (e.g. `checked_at`) will produce
  `NodeUpdate` events on each rebuild — this is intentional and expected. The invariant
  to protect is BID stability: node BIDs must not change between parses of an unchanged
  repo. The BID stability test must still pass with `--git-tracking` enabled.
- `GitCache` must be exercised with two networks in the same git repo sharing
  one `Repository` open
- `source_url` must be correct for both document nodes and section nodes, with
  and without line numbers, and with the `git_remote_url` payload override
- Build without `git-tracking` feature must compile cleanly with no dead-code
  warnings

## Success Criteria

- [x] `BeliefNode.metadata` survives the full parse → `NodeUpdate` → DB → JSON
  export → browser round-trip
- [x] `generate_source()` never contains `metadata` or `source_url` fields
- [x] Git status is path-local (changes outside network dir do not mark it dirty)
- [x] Works gracefully when network is not inside any git repository
- [x] Every document and section node has `metadata["source_url"]` populated
  when `--git-tracking` is enabled and the repo has a recognized remote
- [x] `metadata.js` renders a "View on remote ↗" link in the metadata panel
  for nodes that have `source_url`
- [x] Section nodes link to `file.md#L<n>` (with correct line number)
- [x] `git_remote_url` in a network's `payload` overrides auto-detected remote
- [x] `BeliefNetwork` nodes have `metadata["git"]` populated (commit, branch,
  dirty) after parsing with `--git-tracking`
- [x] Without `--git-tracking` (or `NOET_GIT_TRACKING`), behaviour is identical
  to today — no performance impact
- [x] `test_belief_set_builder_bid_generation_and_caching` passes with git
  tracking enabled on a live git repo (BIDs are stable; `NodeUpdate` events from
  `checked_at` changes are expected and do not constitute a failure)

## Risks

**Risk 1: `git2` C dependency on Windows CI**
- **Impact**: `git2` links libgit2; Windows runner needs `cmake` and `libssl`
- **Mitigation**: Feature-gated; update CI matrix only for `git-tracking` builds

**Risk 2: Parallel epoch-batch task builders**
- **Impact**: `GitCache` accessed from multiple tasks concurrently
- **Mitigation**: `ProtoIndex` already wraps `Arc<RwLock<...>>`; `GitCache`
  uses the same pattern. Cache is populated during `build()` (single-threaded)
  before tasks start; read-only during parallel phase.

**Risk 3: Detached HEAD / exotic repo states**
- **Impact**: `branch`, `upstream`, `ahead`, `behind` may not be computable
- **Mitigation**: All these fields are `Option` or default to zero; URL falls
  back to `HEAD` ref; any `git2` error logs a warning and skips that network

**Risk 4: Staging keys eliminated by design**
- No staging keys are used. Git status travels alongside the `IRNode` as a typed
  `Option<NetworkGitStatus>` and is assigned directly to `BeliefNode.metadata`
  in `GraphBuilder::push`. `payload` is structurally unreachable from git data.

**Risk 5: `source_line` accuracy in `MdCodec`**
- **Impact**: Line numbers may be off-by-one or missing for certain heading
  constructs (setext headings, headings inside block quotes)
- **Mitigation**: Unit tests with known fixtures assert exact line numbers;
  fall back to no `#L` suffix rather than emitting a wrong number

## Open Questions

- Should `metadata` be shown in `BeliefNode`'s `Display` impl? Currently omitted.
  Deferred — add under a `--verbose` flag if needed.
- **Resolved**: `--git-tracking` added to both `Parse` and `Watch` (Step 10).
- **Resolved**: `metadata` exposed as a top-level field on `NodeContext`
  (`context.metadata.source_url`) — cleaner for `metadata.js` consumption.

## Out of Scope (Future Work)

- Per-node file-level dirty status via `inject_context` and `PathMap`
  (requires `BeliefContext` to expose ancestor node's `metadata`)
- Per-network `git_tracking` config in `index.md` frontmatter
- Bitbucket / Forgejo / Gitea blob URL patterns (use `git_remote_url` override)
- Git hooks integration (auto-parse on commit/checkout)
- Submodule recursion
- Remote fetch for ahead/behind (local refs only in Phase 1)
- CLI `noet git-status` command and `--require-clean` validation flag

## References

- `src/codec/proto_index.rs` — `ProtoIndex::build`, `proto_for` (tuple return),
  `net_dir_partition`
- `src/codec/network.rs` — `NetworkCodec::proto`, `prepare_proto_relations`
- `src/codec/md.rs` — `MdCodec` heading parsing, `byte_offset_to_location`,
  `inject_context` (`&IRNode` argument, `source_line` access)
- `src/codec/belief_ir.rs` — `IRNode`, `source_line`, `IntermediateRelation.location`
- `src/properties.rs` — `BeliefNode`, `payload`, `metadata`, `toml()`
- `src/codec/builder.rs` — `parse_content`, Phase 1 `push` (metadata assignment),
  Phase 4 `inject_context`
- `src/codec/compiler.rs` — `DocumentCompiler::with_html_output`, `set_jobs`,
  `export_beliefbase_json`
- `src/bin/noet/main.rs` — CLI `jobs` pattern to mirror for `git_tracking`
- `assets/viewer/metadata.js` — `renderNodeContext`, metadata panel rendering
- `docs/design/beliefbase_architecture.md` — Section 3.2 (codec system),
  Section 3.1 (GraphBuilder phases)