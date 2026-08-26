# BeliefBase Orientation for AI Agents

This document is written for AI agents encountering a noet BeliefBase for the first
time. Read it once, then use the MCP tools to explore the corpus. You do not need to
read any source files directly — the tools give you structured access to everything.

---

## 1. What Is a BeliefBase?

A BeliefBase is a compiled, queryable semantic graph built from a corpus of source
files. Each source file is parsed by a codec into **nodes** connected by typed
**edges**. The compiled output is what you are querying — not the raw files.

Source formats are codec-defined and corpus-specific. Common formats include Markdown
(`.md`), serialized data formats, and even source code. All formats compile into the
same graph structure — the tools work identically regardless of what the source files
look like.

The corpus is organized into **networks**. A network is a directory of source files
that share a common identity (a UUID called a BID and a short 5-hex-char alias called
a bref). Networks are the top-level units of organization. Call `get_networks` first
to enumerate them.

---

## 2. BIDs and Brefs

Every node has two identifiers:

- **BID** — a full UUID (e.g. `"550e8400-e29b-41d4-a716-446655440000"`). Globally
  unique. Use BIDs in tool calls that take a `bid` parameter.
- **bref** — a 5-character hex alias derived from the BID (e.g. `"a3f12"`). Stable
  shorthand. Brefs appear in paths, edge ownership fields, and search results.

BIDs and brefs are interchangeable as node identifiers in most contexts, but tool
inputs expect BIDs. Use `search` to find a node by title or content and get its BID,
then pass that BID to `get_context`.

---

## 3. Graph Direction: Source = Child, Sink = Parent

This is the most important convention to internalize before traversing edges.

**Edges flow from more-specific to less-specific:**

```
source (child, more specific, lower level)
    │
    │  edge (e.g. maps_to, implements, requires)
    ▼
sink (parent, less specific, higher level)
```

Examples:
- A gap analysis entry (source) `maps_to` an external standard requirement (sink).
- A test case (source) `implements` a design requirement (sink).
- A change plan item (source) `requires` a process step (sink).

When you call `get_context` on a node:
- **Sources** of that node are its *children* — nodes that point to it.
- **Sinks** of that node are its *parents* — nodes it points to.

When traversing "upstream" (toward more general concepts), follow sinks.
When traversing "downstream" (toward more specific implementations), follow sources.

---

## 4. Weight Kinds (Edge Types)

Every edge carries one of three **weight kinds**: `Section`, `Epistemic`, or
`Pragmatic`. These are not names for specific relationships — they are three
orthogonal structural axes that together form the full relational picture of any
node. Think of them as three independent dimensions you can read off the same
node simultaneously.

### Section — the structural axis

Section edges define **composition**: what a node is made of, and what it belongs
to. They are the skeleton of the graph.

| Direction | Verb | Meaning |
|-----------|------|---------|
| source→this (in) | *consists of* | This node is composed of those source nodes |
| this→sink (out) | *component of* | This node is a part of that sink node |

Section edges are what `get_submap` traverses. The submap of a network is the set
of all nodes reachable via Section edges from a root — i.e. the structural
decomposition of that network into its constituent parts. This is the **x-axis**
of the traceability matrix: it gives you an ordered, hierarchical row set against
which you can compare the other two dimensions.

### Epistemic — the knowledge axis

Epistemic edges define **justification**: what knowledge a node rests on, and what
it in turn supports.

| Direction | Verb | Meaning |
|-----------|------|---------|
| source→this (in) | *draws from* | This node's reasoning is grounded in those source nodes |
| this→sink (out) | *underlies* | This node provides the basis for that sink node |

In an engineering corpus, Epistemic edges connect requirements to the analyses,
measurements, or design rationale that justify them. In a knowledge corpus they
connect claims to their evidence. They express *why* something is believed.

### Pragmatic — the action axis

Pragmatic edges define **use**: what a node drives or is driven by.

| Direction | Verb | Meaning |
|-----------|------|---------|
| source→this (in) | *uses* | This node is employed or invoked by those source nodes |
| this→sink (out) | *used by* | This node employs or invokes that sink node |

Pragmatic edges express traceability claims: a gap analysis entry uses an external
requirement, a test uses a system requirement, a change plan item uses a gap. They
are the primary edges walked in compliance analysis. In the traceability matrix,
Pragmatic edges populate the columns that show cross-network coverage.

### Relation labels are corpus-specific payload, not weight kinds

Within a weight kind, individual edges carry a corpus-defined relation label in
their payload (e.g. `maps_to`, `implements`, `requires`, `references`). These are
semantic annotations *on top of* the weight kind — they are not a fourth kind.
Call `get_context` on a node and inspect the `edges` field to discover the relation
labels in use in your corpus.

### The matrix view

The three weight kinds are designed to support a matrix reading of any submap:

- **Section** provides the **rows** — the structural decomposition of a scope into
  its constituent nodes, in hierarchical order.
- **Epistemic** and **Pragmatic** provide the **columns** — how many edges of each
  kind and direction each row node has, revealing coverage gaps and connectivity
  patterns across the graph.

This is the same structure the viewer's Traceability modal renders. The
`get_traceability` MCP tool (when available) returns this matrix directly.

---

## 5. Owned Edges and `{maps_to}` Directives

Some edges are **owned** by a third node that is neither the source nor the sink. This
happens when a section uses a `{maps_to}` directive to assert a traceability claim
between two other nodes.

In `get_context` output:
- `edges` — all edges where this node is source or sink
- `owned_edges` — edges where this node is the *owner* (the `{maps_to}` directive
  section), even though it is neither source nor sink

For gap analysis work, the `owned_edges` field is often what you want: it shows all
the traceability claims a gap analysis section is making.

---

## 6. Schemas and `kind` Fields

Nodes carry a `kind` field in their metadata that identifies their schema class.
Examples: `"ext-req"` (external requirement), `"ext-gap"` (gap analysis entry),
`"ext-na"` (not-applicable tag), `"change-plan"` (change plan item).

Use `kind` to filter `query` results. For example, to find all gap analysis entries:

```
{ "query_string": "kind == ext-gap" }
```

Schemas are corpus-specific. Call `get_networks` and then `get_context` on a few
representative nodes to discover the `kind` values in use.

---

## 7. Canonical Tool Sequences

### Orient → Explore

1. `get_networks` — enumerate networks, read the orientation note in the response
2. `get_context <network_bid>` — inspect the network node itself for structure
3. `search <topic>` — find nodes relevant to your task
4. `get_context <node_bid>` — inspect a specific node and its relations

### Verify Anchor Existence

To check whether an `id://some-anchor` reference resolves to a real node:
1. `search "some-anchor"` — find candidate nodes by title or content
2. `get_context <candidate_bid>` — confirm the node exists and read its context
3. If the `check_consistency` tool reports it as unresolved, the anchor is broken

### Traceability Traversal

To trace a gap analysis entry to the external requirement it covers:
1. `get_context <gap_entry_bid>` — read `owned_edges` to find the `maps_to` edge
2. The sink BID of that edge is the external requirement; call `get_context` on it
3. `get_submap <gap_entry_bid> depth=2 direction="downstream"` — see the full
   upstream context (what standards sections surround the requirement)

### Completeness Check (Gap Analysis)

1. `query { "query_string": "kind == ext-req" }` — get all requirements
2. `query { "query_string": "kind == ext-gap" }` — get all gap entries
3. For each requirement BID, call `get_context` and inspect `sources` — any requirement
   with no gap-entry sources is uncovered
4. `check_consistency` — surface broken cross-references and orphaned edges in one call

### Query Examples

The `query` tool accepts a textual query string. Common patterns:

```
-- Section submap from a network root:
{ "query_string": "id://my-network composed_of(*)" }

-- Text search with filter:
{ "query_string": "title:authentication AND schema:procedure" }

-- Composition: items in set A not covered by set B:
{ "query_string": "id:class-a uses(1) NOT id:class-b uses(1)" }

-- Nodes with no outgoing pragmatic edges (inverted traversal):
{ "query_string": "id://my-network composed_of(*) !uses(1)" }

-- Terminal fold to collapse multi-key results:
{ "query_string": "KEYS(id:a,id:b) composed_of(*) FOLD(UNION)" }
```

See `docs/design/query_model.md` §9.5 for the full grammar reference.

### BID ↔ Bref Translation

Tool inputs take full BIDs (UUID format). Tool outputs often include brefs (5-char
hex aliases) in the `network` field of search results and in edge ownership fields.

- **BID → bref**: `bref {"bid": "<uuid>"}` — instant, no BeliefBase lookup.
  Use this when you have a BID and need the short form to match against a known
  network bref or pass to a `network` filter parameter.
- **Bref → BID**: no direct reverse tool. Use `get_networks` to find the BID for a
  network bref, or `search` by title to find the BID for a specific node.

### Orphan and Consistency Detection

1. `check_consistency` — returns `unresolved_refs` (broken `id://` links) and
   `orphaned_edges` (edges with missing endpoints)
2. For each unresolved ref, call `search <raw_key>` to find the intended target
3. For each orphaned edge, call `get_context <source_bid>` to understand the context

---

## 8. Static vs. Live Mode

The MCP server can run in two modes:

- **Static mode** (`--output-dir`): reads pre-compiled shards from a build output
  directory. Fast to start. Results reflect the last `noet parse` run.
  Check `compiled_at` in `check_consistency` output to assess freshness.
- **Live mode** (`--watch`): subscribes to the running compiler. Results always
  reflect the current state of source files, including unsaved edits.

For gap analysis review on a stable corpus, static mode is sufficient.
For active authoring sessions where you are editing and checking simultaneously,
use live mode.

---

## 9. Things Agents Often Get Wrong

**Do not read source files directly.** The tools give you structured, compiled access.
Raw source files (Markdown, YAML, C++ headers, etc.) contain codec-specific syntax —
bref cross-references, TOML frontmatter, codegen macros — that is harder to interpret
than the compiled node context the tools return.

**Do not compare raw BID values across sessions.** BIDs for nodes that have never
been written to disk embed a timestamp and will differ across fresh parse runs.
Compare node titles, brefs, or structural positions instead.

**`get_context` on a network node returns the network's own metadata**, not all the
nodes in the network. To enumerate nodes in a network, use
`query { "query_string": "id://network-name composed_of(*)" }` or `get_submap`
from the network's root path.

**Sinks are parents, not children.** Re-read section 3 if a traversal produces
unexpected results. The source→sink direction is opposite to what "parent points to
child" might suggest in other graph conventions.

**`owned_edges` is the traceability field.** For gap analysis, `edges` shows what
the node is directly connected to; `owned_edges` shows what claims it is asserting
about other nodes. These are complementary views.
