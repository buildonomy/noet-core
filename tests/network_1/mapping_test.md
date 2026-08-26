---
bid = "10000000-0000-0000-0000-000000000030"
schema = "Document"
title = "Mapping Test Document"
---

# Mapping Test Document

This document exercises the `{maps_to}` directive (Issue 61). It contains two
requirement-like nodes and two implementation-like nodes, then a section that
uses `{maps_to}` to declare directed edges between them — owned by the section
rather than by either endpoint.

## Requirement Alpha {#req-alpha}

This section represents a requirement node. In noet semantics, requirements are
**sources** — they are upstream dependencies that implementors draw from.

## Requirement Beta {#req-beta}

A second requirement node. Used to test that a `{maps_to}` directive with
multiple sources produces one edge per (source, sink) pair — the Cartesian
product of sources × sinks.

## Implementation One {#impl-one}

This section represents an implementation artefact that satisfies the
requirements above. In noet semantics, implementors are **sinks** — they
receive the dependency flow from the upstream requirement sources.

## Trace Mapping

This section owns the mapping edges. It is neither a source nor a sink
endpoint — it merely declares the relationships between the nodes above.

The `{maps_to}` directive body is parsed as TOML (with YAML/JSON fallback).
The info-string arg sets the weight kind (`Pragmatic`). `source` holds the
requirement nodes (upstream dependencies); `sink` holds the implementor nodes
(downstream, what receives the dependency). Each field accepts a single string
or an array; one edge is emitted per element of the Cartesian product
`sources × sinks`.

````{maps_to} Pragmatic
source = ["id://req-alpha", "id://req-beta"]
sink = "id://impl-one"
````

The rendered output of the directive above is replaced in-place by a mapping
table listing the source, kind, and each sink with their resolved titles.
