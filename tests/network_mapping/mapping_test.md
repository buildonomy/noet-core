---
bid = "10000000-0000-0000-0000-000000000030"
schema = "Document"
title = "Mapping Test Document"
---

# Mapping Test Document

This document exercises the `{maps_to}` directive (Issue 61). It contains two
requirement-like nodes and one implementation-like node, then a section that
uses `{maps_to}` to declare directed edges between them — owned by the section
rather than by either endpoint.

## Requirement Alpha {#req-alpha}

This section represents a requirement node. In noet semantics, requirements are
**sources** — they are upstream nodes whose outputs flow into downstream sinks.

## Requirement Beta {#req-beta}

A second requirement node. Used to test that a `{maps_to}` directive with
multiple sources produces one edge per (source, sink) pair — the Cartesian
product of sources × sinks — and that the rendered table correctly rowspans the
shared sink cell across those rows.

## Implementation One {#impl-one}

This section represents an implementation artefact that satisfies the
requirements above. In noet semantics, implementors are **sinks** — they are
downstream nodes that receive the dependency flow from upstream sources.

## Trace Mapping

This section owns the mapping edges. It is neither a source nor a sink
endpoint — it merely declares the relationships between the nodes above.

The `{maps_to}` directive body is parsed as TOML (with YAML/JSON fallback).
The info-string arg sets the weight kind (`Pragmatic`). `source` holds the
upstream nodes; `sink` holds the downstream nodes that receive the dependency
flow. Each field accepts a single string or an array; one edge is emitted per
element of the Cartesian product `sources × sinks`.

````{maps_to} Pragmatic
source = ["id://req-alpha", "id://req-beta"]
sink = "id://impl-one"
````

The rendered output of the directive above is replaced in-place by one table
per WeightKind present in the directive. Each table has a `<caption>` showing
the kind name and two columns: **Sink** then **Source**. When multiple sources
map to the same sink, the sink cell spans all of their rows via `rowspan`.
For the directive above this produces a single `Pragmatic` table with two rows
(one per source), the `impl-one` sink cell spanning both.