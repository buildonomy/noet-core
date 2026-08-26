---
version = "0.1"
title = "noet MCP Server"
---

# noet MCP Server

The `noet mcp` subcommand starts a [Model Context Protocol](https://modelcontextprotocol.io)
server that exposes a compiled BeliefBase as a structured query engine for AI agents.

A BeliefBase is a compiled semantic graph: source files (Markdown, YAML, code, or any
codec-supported format) are parsed into **nodes** connected by typed **edges**. The MCP
server gives agents direct, structured access to this graph — searching by content,
traversing relationships, inspecting traceability claims, and checking structural
consistency — without reading raw source files.

The `noet://help/orientation` MCP resource (auto-injected by compatible clients)
contains the agent-facing version of this documentation, tuned for an LLM audience.

---

## Installation

The `mcp` feature is opt-in and not included in the default `noet` binary. Install
with:

```bash
cargo install --git <repo-url> --features mcp
```

Or via a downstream project's Makefile:

```bash
make deps        # installs noet with --features mcp
make update-noet # force-reinstall to latest with mcp feature
```

To build locally from source:

```bash
cargo build --features mcp
# binary at: ./target/debug/noet
```

---

## Modes

### Static mode — `--output-dir`

Loads pre-compiled shards from a `noet parse --html-output` output directory. Fast
to start. Results reflect the last parse run.

```bash
noet mcp --output-dir /path/to/_site
```

Use this for:
- Gap analysis review on a stable corpus
- CI pipelines that parse once, then run agent checks
- Situations where you do not need live recompilation

Results are as fresh as the last `noet parse`. Check `compiled_at` in
`check_consistency` output to assess staleness (requires Issue 66).

### Live mode — `--watch`

Spins up a `WatchService`, waits for the first compile pass to complete, then serves
queries against the live in-memory database. Results always reflect the current state
of source files.

```bash
noet mcp --watch /path/to/corpus
```

With search index support (required for the `search` tool in live mode):

```bash
noet mcp --watch /path/to/corpus --html-output /path/to/_site
```

Use this for:
- Active authoring sessions where you are editing and checking simultaneously
- LSP-adjacent workflows where freshness matters

**Note**: The `search` tool requires `.idx.msgpack` files produced by
`noet parse --html-output`. In live mode without `--html-output`, `search` returns
empty results gracefully. All other tools (`get_context`, `query`, `get_networks`,
etc.) work without `--html-output`.

---

## Claude Desktop configuration

Add to `~/Library/Application Support/Claude/claude_desktop_config.json`
(macOS) or the equivalent on your platform:

```json
{
  "mcpServers": {
    "my-corpus": {
      "command": "noet",
      "args": ["mcp", "--output-dir", "/Users/you/code/my-corpus/_site"]
    }
  }
}
```

For live mode:

```json
{
  "mcpServers": {
    "my-corpus-live": {
      "command": "noet",
      "args": [
        "mcp",
        "--watch", "/Users/you/code/my-corpus",
        "--html-output", "/Users/you/code/my-corpus/_site"
      ]
    }
  }
}
```

Restart Claude Desktop after editing the config. The server name (e.g.
`"my-corpus"`) is arbitrary — use something that identifies the corpus.

---

## Tools

All tools are called via the MCP `tools/call` method. Tool inputs use UUID BIDs; the
`bref` tool converts a BID to its 5-char hex alias.

| Tool | Purpose |
|------|---------|
| `get_networks` | List all compiled networks with stats and an orientation note. **Call this first.** |
| `search` | Full-text TF-IDF search. Returns BIDs, titles, scores, paths. |
| `get_context` | Full relationship context for a node: sources, sinks, edges, owned edges. |
| `get_submap` | Section-edge structural subgraph rooted at a BID. |
| `query` | Execute a query using the textual grammar. Returns matching nodes and edges. |
| `check_consistency` | Surface unresolved cross-references and orphaned edges. |
| `get_traceability` | Direct edge-count matrix for a structural submap (rows × 6 columns). |
| `get_maps_to` | Flat list of `{maps_to}` claims for a set of owner BIDs. |
| `get_maps_to_traceability` | Full three-level claim index: owner → sink → {kind: [sources]}. |
| `bref` | Convert a BID (UUID) to its 5-char hex bref alias. Pure computation. |

### Key tool parameters

**`get_context`**
```json
{ "bid": "<uuid>", "include_content": false }
```
Set `include_content: true` to include the node's full source text. Default `false`
keeps token usage low for large document nodes.

**`search`**
```json
{ "query": "thermal control", "limit": 20, "network": "<bref>" }
```
`network` is optional — omit to search all networks.

**`get_submap`**
```json
{ "bid": "<uuid>", "depth": 0, "direction": "both" }
```
`depth: 0` returns the immediate level (subnets opaque). `depth: 255` is fully
recursive. `direction` is `"upstream"`, `"downstream"`, or `"both"`.

**`get_traceability`**
```json
{ "bid": "<uuid>", "depth": 0, "weight_kinds": ["section", "epistemic", "pragmatic"] }
```
Returns rows with `section_in`, `section_out`, `epistemic_in`, `epistemic_out`,
`pragmatic_in`, `pragmatic_out` counts. Use `depth > 0` to expand subnets.

**`get_maps_to`**
```json
{ "bids": ["<uuid>", "<uuid>"], "weight_kinds": ["pragmatic"] }
```
Pass the BIDs of owner nodes (e.g. gap analysis sections). Returns flat
`(owner, source, sink, kind)` claim tuples. Cheaper than
`get_maps_to_traceability` — no submap resolution required.

**`get_maps_to_traceability`**
```json
{ "bid": "<uuid>", "depth": 0, "weight_kinds": ["pragmatic"] }
```
Resolves the structural submap from `bid`, then builds the full
owner → sink → {kind: [sources]} index. The compliance review primitive.

**`query`**
```json
{ "query_string": "id://my-network composed_of(*)", "network": "<bref>" }
```
Accepts a textual query string (see `docs/design/query_model.md` §9.5 for the
full grammar). `network` is optional — omit to query all networks. Examples:

```json
{ "query_string": "title:auth AND schema:procedure" }
{ "query_string": "id:class-a uses(1) NOT id:class-b uses(1)" }
{ "query_string": "KEYS(id:a,id:b) composed_of(*) FOLD(UNION)" }
{ "query_string": "id://my-network composed_of(*) !uses(1)" }
```

---

## Resources

The server exposes two resource surfaces, accessible via `resources/list` and
`resources/read`:

**`noet://help/orientation`** — LLM-targeted orientation document covering BIDs,
brefs, graph direction, weight kinds, owned edges, and canonical tool sequences.
Annotated `audience: ["assistant"]`, `priority: 0.9` so MCP-aware clients
(Claude Desktop) auto-inject it as system context on every session.

**`noet://help/{name}`** — Any `docs/design/*.md` file from the noet-core source
tree, served by filename stem with TOML frontmatter stripped. Examples:

```
noet://help/beliefbase_architecture
noet://help/search_and_sharding
noet://help/link_format
```

Use these to pull authoritative design documentation into an agent session on demand.

---

## Graph conventions (essential)

**Source = child, sink = parent.** Edges flow from more-specific to more-general:

```
source (child, more specific)  →  sink (parent, more general)
```

- A requirement (source) `implements` a higher-level requirement (sink).
- A gap analysis entry (source) `maps_to` an external standard section (sink).
- `get_context` returns `sources` (nodes pointing TO this one) and `sinks` (nodes
  this one points TO).

**Three weight kinds** — orthogonal structural axes, not names for specific
relationships:

| Kind | Axis | In-verb | Out-verb |
|------|------|---------|----------|
| `section` | Structure | *consists of* | *component of* |
| `epistemic` | Knowledge | *draws from* | *underlies* |
| `pragmatic` | Action | *uses* | *used by* |

**Owned edges** — some edges are declared by a third node via `{maps_to}`. For
compliance analysis, `get_context` returns `owned_edges` showing what claims a
section node is asserting about other nodes, even though it is neither source nor
sink of those edges.

---

## Typical agent session sequence

```
1. get_networks {}
   → orient: read the orientation note, note the network brefs

2. search { "query": "thermal control", "limit": 10 }
   → find relevant nodes by keyword

3. get_context { "bid": "<result-bid>" }
   → inspect node, follow sources/sinks, read owned_edges for traceability claims

4. get_maps_to_traceability { "bid": "<network-bid>", "depth": 1 }
   → full compliance picture for a traceability network

5. check_consistency {}
   → surface broken cross-references and orphaned edges
```

Corpus-specific methodology is best provided via `AGENTS.md` in the corpus root or
injected as a user message before the agent begins.

---

## Known limitations

**Static mode `get_maps_to*` requires `--features mcp` binary.** The tools work
correctly when the binary is built with the `mcp` feature. An older binary without
this feature will silently route unknown tool names to an error arm.

**`compiled_at` is not yet populated.** The `check_consistency` response includes a
`compiled_at` field but it is currently `null`. It will be populated once Issue 66
adds `compiled_at` to `NetworkShardMeta`.

**`check_consistency` live-mode diagnostic enrichment is deferred.** The current
implementation detects unresolved refs by walking the graph (edges whose sink BID is
absent). Compiler-reported `ParseDiagnostic::UnresolvedReference` entries (which
include file paths and line numbers) will be added in Issue 66 via
`DocumentCompiler::last_diagnostics()`.

**Search snippet field is empty.** `search` returns `snippet: ""` for all results.
Populating snippets requires loading the node's shard text into the result; deferred
for a later pass.

**`noet://help/{name}` serves noet-core design docs, not corpus docs.** The resource
template serves `docs/design/*.md` from the noet-core source tree — architecture and
design documents. It does not serve documents from the corpus being queried.

---

## Troubleshooting

**`noet: command not found`** — Run `make deps` from the downstream project's
directory, or `cargo install --git <repo> --features mcp`.

**Server starts but `get_networks` returns empty list** — Ensure you are running
a binary built from the latest source. Older binaries did not populate the manifest
for monolithic corpora (below shard threshold). The current code scans the loaded
BeliefBase for network nodes and builds the manifest automatically.

**`search` returns empty results** — In live mode without `--html-output`, search
index files (`.idx.msgpack`) are not present. Add `--html-output /path/to/_site` to
the `--watch` invocation, or run `noet parse --html-output /path/to/_site` first.

**`get_maps_to*` returns empty results in static mode** — This is a known shard
format limitation. The `WEIGHT_OWNED_BY` field in edge weight payloads is stored via
`#[serde(flatten)]` on a `toml::Table`, and the static mode shard files are loaded
via `rmp_serde` which does not reliably round-trip flattened TOML maps. As a result,
the owned-by bref is absent from edge weights loaded from `.msgpack` shards, and the
full-graph scan in `get_maps_to` finds no matches.

**Workaround**: use live mode (`noet mcp --watch`) instead of static mode. In live
mode the `DbConnection` is built from streamed `BeliefEvent`s, which carry the
correct weight payloads without a msgpack round-trip. `get_maps_to` and
`get_maps_to_traceability` work correctly in live mode.

Also occurs if the corpus has no `{maps_to}` directives, or if the binary was built
without `--features mcp`. Rebuild with `cargo build --features mcp` if unsure.

**Claude Desktop does not inject `noet://help/orientation`** — Not all MCP clients
auto-inject resources annotated `audience: ["assistant"]`. The `get_networks`
response always includes an inline `orientation` field as a fallback — the agent
gets baseline orientation from the first tool call regardless of resource injection.
