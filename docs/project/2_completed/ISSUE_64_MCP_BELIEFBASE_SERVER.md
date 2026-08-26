---
title = "Issue 64: MCP BeliefBase Server"
version = "0.1"
---

# Issue 64: MCP BeliefBase Server

**Status**: COMPLETE (static mode); live mode deferred
**Priority**: HIGH
**Estimated Effort**: 6 days (RELATIVE COMPARISON ONLY)
**Dependencies**: Requires Issue 50 (shard export), Requires Issue 54 (search query in WASM — for shared `src/shard/search.rs` extraction); Informs Issues 11/12 (LSP)

## Summary

AI agents working on complex multi-system projects (e.g., a systems-engineering corpus, a QMS corpus) currently operate with shallow, ad hoc context — reading individual files rather than querying a coherent semantic graph. This issue adds a Model Context Protocol (MCP) server built on top of `WatchService` that exposes the compiled noet BeliefBase as a rich semantic query engine for agents. Agents gain structured context retrieval, full-text search, relationship traversal, and inconsistency surfacing — all backed by the live compiler pipeline, without reading raw source files.

## Completion Notes

Static mode (`noet mcp --output-dir <path>`) is fully implemented. Key deviations from the original plan:

- **Ten tools implemented** (not six): `get_networks`, `search`, `get_context`, `get_submap`, `query`, `check_consistency`, plus `bref`, `get_traceability`, `get_maps_to`, `get_maps_to_traceability`.
- **Resource URI scheme changed**: `noet://docs/*` → `noet://help/*` to better reflect the LLM-oriented content.
- **`gap_analysis_review` prompt dropped**: application-specific prompts belong in the corpus (e.g. a systems-engineering corpus, a QMS corpus), not the library. The `get_networks` orientation note provides baseline agent orientation.
- **`check_consistency` derives from graph directly**: does not use `DocumentCompiler::last_diagnostics()`. The `last_diagnostics` accessor is still a TODO for live mode (Issue 11 / future Step 2).
- **`compiled_at` in `check_consistency`**: returns `None` pending Issue 66 (`NetworkShardMeta.compiled_at`).
- **Live mode (`--watch`) deferred**: subscribing to `FileUpdateSyncer::belief_broadcast` is out of scope for this issue; left for a follow-on.
- **`BeliefContext::all_owned_edges()`** implemented in `src/beliefbase/context.rs`; both `wasm.rs` and `src/mcp/tools.rs` call it — no duplication.

The motivating use case is gap analysis review (see `vast_qms/src/gap_analysis/review_plan.md`): a structured 7-check review against an external compliance standard that requires cross-referencing hundreds of `{maps_to}` edges, verifying anchor existence, checking tag defensibility, and surfacing orphaned change plan entries. This review is mechanical enough to be agent-automatable — but only if the agent has structured, graph-aware access to the corpus instead of reading files ad hoc.

## Goals

- Expose `BeliefBase` query semantics as MCP tools via a local stdio/HTTP server
- Build on `WatchService` so agents always query a live, recompile-on-change graph
- Enable structured context retrieval, full-text search, edge traversal, and consistency checking
- Reuse `src/shard/search.rs` (already native Rust) for query-time TF-IDF — no duplication
- Establish the query-over-live-graph pattern that LSP (Issues 11/12) will also consume
- Enable agent-driven gap analysis review: completeness checking, anchor resolution, edge traversal, orphan detection

## Architecture

### Why MCP

MCP (Model Context Protocol) is the emerging standard for tool-augmented LLM workflows.
Claude, Cursor, and other agent runtimes natively support MCP servers via stdio transport.
Implementing MCP eliminates per-agent integration work: any MCP-capable agent gets
BeliefBase access without bespoke glue code.

MCP servers expose **tools** (callable functions with typed schemas). The BeliefBase maps
cleanly onto a small, high-value tool set:

| MCP Tool | BeliefBase operation |
|---|---|
| `get_networks` | Enumerate compiled networks with stats |
| `search` | TF-IDF search across all loaded `SearchIndex` instances |
| `get_context` | Node + related nodes + typed edges (mirrors `extract_node_context`) |
| `get_submap` | Pragmatic-edge traceability subgraph (mirrors `get_submap` in `wasm.rs`) |
| `query` | Raw `Expression` query against `BeliefBase::evaluate_expression` |
| `check_consistency` | Orphan detection, broken cross-refs, summary |

### Building on WatchService

The MCP server is a subscriber to an existing `WatchService` / `FileUpdateSyncer`, not
a standalone loader. This gives live recompilation for free:

```
Source Files
    │
    ▼
WatchService / FileUpdateSyncer
    ├── DocumentCompiler (recompiles on file change)
    ├── belief_broadcast: broadcast::Sender<BeliefEvent>   ← already exists, marked
    │                                                          "forward-looking API for LSP"
    └── compiler_idle_notify: Arc<Notify>                  ← already exists, same note
    │
    ▼
MCP Server (new: src/mcp/)
    ├── Subscribes to belief_broadcast → keeps in-memory BeliefBase current
    ├── Listens on stdin (stdio transport) or TCP (HTTP/SSE transport)
    └── Serves tool calls against the live BeliefBase
```

`FileUpdateSyncer.belief_broadcast` and `compiler_idle_notify` are explicitly marked
"forward-looking API for LSP" in `watch.rs`. The MCP server is their first consumer;
Issue 11 follows the same pattern.

**Static mode** (`noet mcp --output-dir <path>`): loads shard JSON directly via
`src/shard/` wire types, no running compiler required.

### Native Rust, Not WASM

The MCP server calls `BeliefBase` methods directly; `BeliefBaseWasm` documents the
semantics but the WASM bindings are not used.

The native API is already in the right place:

- `BeliefBase::get_context()` → `BeliefContext` in `src/beliefbase/context.rs` — MCP
  calls this directly. No extraction needed.
- `PathMapMap::submap_by_bid()` — MCP calls this directly. No extraction needed.
- `query_search_index()` in `src/shard/search.rs` — already target-agnostic (✓ done).

`wasm.rs::extract_node_context` is **not** logic — it is a JS serialization adapter:
`toml::Table → serde_json::Value`, `Reflect::set` patches for JS Map vs. plain-object
semantics, and sorted-graph construction for JS Map consumers. MCP does not inherit any
of this. MCP defines its own output types in `src/mcp/types.rs`.

The one piece of business logic that has drifted into `wasm.rs` and belongs natively is
the `owned_edges` + `declared_edges` deduplication/merge (building the
`(source, sink, weight_kind) → owner_bid` lookup index). This should move to
`BeliefContext::all_owned_edges()` in `context.rs` so both `wasm.rs` and `src/mcp/`
consume it without duplication.

### Convergence with LSP (Issues 11 / 12)

LSP and MCP share the same subscriber infrastructure (`belief_broadcast`,
`compiler_idle_notify`) and differ only in wire protocol and query surface (position-
anchored cursor operations for LSP vs. graph-query batch operations for MCP). They are
complementary phases of the same authoring workflow: MCP for analytical passes (agent
queries corpus), LSP for remediation passes (human fixes inline).

**Sequencing**: MCP first — validates the subscriber model and the native API surface.
LSP inherits both. Neither blocks the other.

**Shared search**: add the TF-IDF query path to `src/shard/search.rs` alongside the
existing builder; both `wasm.rs` and `src/mcp/` import it without duplication.

### Prompts and Resources

MCP exposes three primitives: **tools** (callable functions), **resources** (context for language models), and **prompts** (user/agent-invoked templates). Tools cover the query surface; resources and prompts handle agent orientation.

**Resources** are application-driven — host clients (Claude Desktop, Cursor) can auto-inject them as system context on every session. Two resources are exposed:

- **`noet://docs/orientation`** — a purpose-written 300–400 line document targeting an LLM audience, covering: what BIDs and brefs are, what source=child/sink=parent means in the relationship graph (most-dependent nodes are deepest sinks), what schemas signal, how to navigate with `get_context` → follow edges → `get_submap`, and 3–4 canonical tool sequences for common agent tasks (search → orient → traverse → check). This is *not* a summary of existing design docs — it is written specifically for agents encountering the corpus for the first time. Stored at `src/mcp/orientation.md`, compiled into the binary.

- **`noet://docs/{name}`** — a resource template serving raw text of any `docs/design/*.md` file by name (e.g., `noet://docs/beliefbase_architecture`). Lets an agent fetch a specific design doc on demand without filesystem access. Lists available names via `resources/list`. Annotated `audience: ["assistant"]`.

**Prompts** are user/agent-invoked. One prompt for the motivating use case:

- **`gap_analysis_review`** — arguments: `{ network: string (the gap analysis network bref), standard: string (the external standard network bref) }`. Returns a structured system message explaining the 7-check review protocol from `vast_qms/src/gap_analysis/review_plan.md` and the recommended MCP tool sequence for each check. Invoking this prompt bootstraps an agent gap analysis session with full methodology context without requiring the agent to read any source file.

**Fallback orientation in `get_networks`**: since prompt invocation is user-controlled and resource injection depends on client behavior, `get_networks` always embeds a brief orientation note (3–5 lines) in its response: what BIDs/brefs are, the source/sink direction convention, and which tools to call next. This guarantees a minimal orientation is always in context after the first tool call, regardless of whether the client injects the resource.

### Tool Specifications

#### `get_networks`
```
Input:  {}
Output: {
    networks: [{ bref, bid, title, node_count, relation_count, path }],
    orientation: string   // brief inline hint: BID/bref explanation, source=child/sink=parent,
                          // suggested next tools — always present regardless of resource injection
}
```

#### `search`
```
Input:  { query: string, limit?: number, network?: string }
Output: [{ bid, title, snippet, score, network, path, loaded: bool }]
```
TF-IDF over pre-built `.idx.msgpack` files (msgpack format; see Issue 54 migration).
Snippet extracted from `payload["text"]` for loaded nodes; empty for unloaded.
Same ranking algorithm as Issue 54.

#### `get_context`
```
Input:  { bid: string }
Output: NodeContext { node, home_net, root_path, metadata, related_nodes,
                     graph, owned_edges }
```

#### `get_submap`
```
Input:  { bid: string, depth?: number, direction?: "upstream"|"downstream"|"both" }
Output: { nodes: [...], edges: [...] }
```
Pragmatic-edge traceability subgraph. Depth defaults to 3.

#### `query`
```
Input:  { expression: Expression }
Output: BeliefGraph { states: {...}, relations: {...} }
```

#### `check_consistency`
```
Input:  { network?: string }
Output: {
    unresolved_refs: [{ path, source_bid, other_keys, weight_kind, location }],
    orphaned_edges:  [{ edge_bid, source_bid, sink_bid, reason }],
    summary: string,
    compiled_at: string
}
```
- **Unresolved refs**: read from the compiler's retained `ParseDiagnostic::UnresolvedReference`
  set — not re-derived by walking the graph. `DocumentCompiler.latest_results` is drained
  by `parse_all` / `parse_sequential` after each pass; the compiler must snapshot it first,
  exposed as `last_diagnostics() -> &HashMap<PathBuf, Vec<ParseDiagnostic>>`. This is also
  the correct source for LSP `publishDiagnostics` (Issue 11) — one accessor, two consumers.
- **Orphaned edges**: edges whose source or sink BID is absent from all loaded networks —
  a graph-level check the compiler does not produce, so derived from the graph directly.

`compiled_at` lets agents reason about staleness in static mode. Schema-level missing-field
detection is deferred (Issue 32 backlogged; `vast_qms` does not yet rely on it).

### Motivating Use Case: Gap Analysis Review

`vast_qms/src/gap_analysis/review_plan.md` defines a 7-check review protocol for
compliance gap analyses (e.g., Vast vs. NPR 7150.2D). Each check maps directly to MCP
tool calls:

| Review Check | MCP Tool(s) |
|---|---|
| Check 1 — All requirements present | `search` by schema/kind + `get_networks` to enumerate external standard nodes |
| Check 2 — Every `id://` sink resolves | `query` for all Pragmatic edges; `get_context` on each sink BID to verify existence |
| Check 3 — Cited sections are relevant | `get_context` on each sink + `get_submap` to see what else the cited section covers |
| Check 4 — Tag defensibility | `query` for nodes tagged `ext-na`/`ext-t`; read prose via `get_context` |
| Check 5 — Gap↔change plan consistency | `check_consistency` orphan detection; `query` for `ext-gap` nodes; cross-ref via `get_submap` |
| Check 6 — Prose rationale quality | `get_context` to read full node content for agent review |
| Check 7 — noet structural validity | `check_consistency` broken-ref detection; unresolved edges surfaced directly |

An agent with this MCP server can execute Checks 1, 2, 5, and 7 **mechanically** —
querying the graph rather than reading files — and produce a dispositioned finding list
following the `OK / EDIT-MINOR / EDIT-TAG / ANCHOR-NEEDED / ...` codes defined in the
review plan. Checks 3, 4, and 6 require judgment but are dramatically accelerated when
the agent can pull precise context rather than scanning files.

### Integration Pattern

```bash
noet mcp --output-dir vast_qms/_site   # static mode
cd vast_qms && noet mcp --watch        # live mode (preferred during active review)
```

Typical agent gap-analysis session: `get_networks` to orient → `check_consistency` to
surface broken sinks and orphaned edges → `query` for all Pragmatic `maps_to` edges →
`get_submap` on an `ext-gap` node to find covering change plan entries → `get_context`
on cited section BIDs to verify relevance → produce dispositioned finding list.

## Implementation Steps

1. **MCP crate integration and server skeleton** (1 day)
   - [x] `rmcp` crate selected; stdio transport implemented
   - [x] `src/mcp/mod.rs` created with tool registry
   - [x] `noet mcp --output-dir` CLI subcommand implemented
   - [x] MCP `initialize` handshake verified with Claude

2. **Subscriber wiring to WatchService** (0.5 days)
   - [ ] Live mode deferred — static mode only for this issue

3. **Confirm native API surface and fix owned-edge drift** (0.25 days)
   - [x] `BeliefBase::get_context()` and `PathMapMap::submap_by_bid()` confirmed sufficient
   - [x] `BeliefContext::all_owned_edges()` added to `src/beliefbase/context.rs`
   - [x] `src/mcp/types.rs` defines MCP output types independently of `wasm.rs`
   - [x] TF-IDF query path in `src/shard/search.rs` (done in Issue 54)

4. **Implement core MCP tools** (1.5 days)
   - [x] `get_networks` with orientation note
   - [x] `search` via `query_search_index` from `shard::search`
   - [x] `get_context`, `get_submap`, `query`, `check_consistency`
   - [x] Additional tools: `bref`, `get_traceability`, `get_maps_to`, `get_maps_to_traceability`

5. **Prompts and resources** (0.5 days)
   - [x] `src/mcp/orientation.md` written and compiled into binary
   - [x] `noet://help/orientation` resource exposed (`audience: ["assistant"]`, `priority: 0.9`)
   - [x] `noet://help/{name}` resource template serving `docs/design/*.md` files
   - [ ] `gap_analysis_review` prompt — **dropped by design**: application-specific prompts belong in the corpus, not the library

6. **Consistency checker and polish** (1 day)
   - [x] `check_consistency` implemented (derives from graph; orphaned edges + unresolved refs)
   - [x] `summary` string in output
   - [ ] `compiled_at` — pending Issue 66 (`NetworkShardMeta.compiled_at`); returns `None`
   - [ ] `last_diagnostics` from `DocumentCompiler` — deferred to live mode (Issue 11)
   - [x] Typed JSON schemas for all tool inputs
   - [x] `docs/mcp.md` documents `noet mcp` usage

## Testing Requirements

- [x] `noet mcp --output-dir <fixture>` passes MCP `initialize` handshake with `tools` and `resources` capabilities declared
- [x] `resources/list` returns `noet://help/orientation` and at least one `noet://help/{name}` entry for every file in `docs/design/`
- [x] `resources/read` on `noet://help/orientation` returns non-empty text containing "BID", "bref", and "source"
- [ ] `prompts/list` returns `gap_analysis_review` — **dropped by design**
- [x] `get_networks` response includes an `orientation` field with non-empty text
- [x] `search "term"` returns results consistent with `BeliefBaseWasm.search` on same output
- [x] `get_context <bid>` matches `BeliefBaseWasm.get_context` on same output
- [x] `check_consistency` returns broken refs against a fixture with a known unresolved sink
- [ ] Live mode: editing a source file triggers recompile — deferred

## Success Criteria

- [x] `noet mcp --output-dir <path>` connects to Claude and passes handshake with tools and resources
- [x] `noet://help/orientation` resource injected by Claude Desktop; agent identifies source=child/sink=parent direction
- [x] Agent can orient via `get_networks`, enumerate edges (`query`), verify anchor existence (`get_context`) without reading source files
- [x] `check_consistency` surfaces known unresolved sinks
- [ ] Agent completes gap analysis Checks 1, 2, 5, 7 — not verified; `gap_analysis_review` prompt dropped
- [x] All tool inputs have typed JSON schemas
- [x] `BeliefContext::all_owned_edges()` in `src/beliefbase/context.rs`; both `wasm.rs` and `src/mcp/` call it
- [x] `src/mcp/types.rs` defines MCP output types independently of WASM serialization types
- [x] `src/shard/search.rs` TF-IDF query path with Levenshtein fuzzy matching
- [x] `src/mcp/orientation.md` compiled into binary; covers BID/bref, graph direction, schema, tool sequences
- [ ] `gap_analysis_review` prompt — dropped by design

## Risks

- **Orientation doc staleness**: `src/mcp/orientation.md` is hand-written and will drift from `docs/design/` as the system evolves. → **Mitigation**: Reference it in `CONTRIBUTING.md` as a doc that must be reviewed when architecture changes. The `noet://docs/{name}` resource template gives agents a path to the authoritative design docs when the orientation summary is insufficient.
- **Resource injection is client-dependent**: Not all MCP clients respect `audience: ["assistant"]` or auto-inject resources. → **Mitigation**: The `get_networks` orientation field guarantees baseline orientation regardless of client behavior.
- **`rmcp` crate maturity**: MCP Rust libraries are early-stage. The MCP JSON-RPC
  protocol is thin enough (~200 lines) that a direct stdio implementation is a viable
  fallback. → **Mitigation**: Audit `rmcp` before committing; keep the fallback path
  explicit in the implementation plan.
- **`wasm.rs` owned-edge drift**: The `owned_edges` + `declared_edges` merge logic in
  `wasm.rs::extract_node_context` is the only business logic that has drifted out of
  `src/beliefbase/`. → **Mitigation**: Moving it to `BeliefContext::all_owned_edges()`
  is a small, self-contained addition with no behavioral change; update `wasm.rs` to
  call it and run existing WASM tests to confirm no regression.
- **Diagnostic retention change to `DocumentCompiler`**: Adding `last_diagnostics`
  snapshot touches a core compiler struct shared across many call sites. → **Mitigation**:
  The change is additive (a new field + accessor, no behavior change to existing callers).
  The drain in `parse_all` / `parse_sequential` is unchanged; the snapshot is taken
  immediately before it. Note that Issue 11 (LSP `publishDiagnostics`) needs this same
  accessor — coordinate so it is added once, not twice.
- **Search index staleness in static mode**: If the agent runs against an old
  `noet parse` output, results may not reflect recent edits. → **Mitigation**:
  `check_consistency` and `search` both include `compiled_at` in output so agents can
  reason about freshness. Document that live mode (`--watch`) is preferred for active
  editing sessions.

## Open Questions

- Should `get_context` inline full `payload["text"]` by default, or gate it behind
  `include_content: bool`? Full inline is simpler but inflates token usage for large
  documents. Recommended default: `false`, with the field available on request.
- Should `noet://docs/{name}` serve files verbatim (including TOML frontmatter) or strip frontmatter for cleaner agent consumption? Verbatim is simpler; stripping adds a preprocessing step. Recommended default: strip frontmatter.
- Should the `gap_analysis_review` prompt embed the full 7-check protocol text or reference it via an embedded `noet://docs/gap_analysis_review_plan` resource? Embedded text is self-contained; resource reference keeps the prompt lean but adds a round-trip. Decide at implementation time based on `rmcp` prompt/resource embedding support.
- Should additional networks be loadable on demand in static mode? Deferred unless the `vast_qms` use case requires it.
- Transport: stdio is sufficient for single-agent use. HTTP/SSE enables multiple
  simultaneous agent connections against the same live graph. Defer HTTP transport
  until there is a concrete multi-agent use case.

## References

- `src/mcp/orientation.md` — LLM-targeted orientation doc (to be created in step 5)
- `src/watch.rs` — `WatchService`, `FileUpdateSyncer`, `belief_broadcast`, `compiler_idle_notify`
- `src/codec/compiler.rs` — `DocumentCompiler.latest_results`, `ParseResult.diagnostics`
- `src/codec/diagnostic.rs` — `ParseDiagnostic::UnresolvedReference`
- `src/shard/search.rs` — `SearchIndex`, `tokenize`, `Stemmer` (TF-IDF query path to be added)
- `src/shard/wire.rs` — shard wire types (usable natively, not wasm32-only)
- `src/wasm.rs` — `extract_node_context` (JS serialization adapter; owned-edge merge to move to `context.rs`)
- `src/beliefbase/context.rs` — `BeliefContext`, `OwnedEdge` (native API, already correct)
- `docs/design/search_and_sharding.md`; `docs/design/beliefbase_architecture.md`
- Issue 11 (LSP) — shares subscriber pattern and `last_diagnostics` accessor
- Issue 50 (Sharding) — shard format; Issue 54 (Search) — shared TF-IDF logic
- MCP spec: https://modelcontextprotocol.io/docs/concepts/prompts — Prompts primitive
- MCP spec: https://modelcontextprotocol.io/docs/concepts/resources — Resources primitive
