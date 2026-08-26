# Query Directive Test

This document is the TDD fixture for the `{query}` directive (Issue 81).
It defines the syntax and expected output shapes that the integration tests
verify. The `{query}` blocks below produce `<!--@@noet-query:N@@-->` sentinels
once the directive is registered; the deferred pipeline in
`generate_html_for_path` replaces each sentinel with rendered HTML.

## Implicit Anchor — default view

No `id://` anchor: the parser produces `Subject::Implicit`, which Phase 3
resolves to this document's own BID before evaluation.

````{query}
:view: depth0
:caption: Nodes connected to this document
k-pragmatic-s(1)
````

## Explicit Anchor — depth0 view

Explicit `id://` anchor pins the seed to a specific node regardless of which
document embeds this directive. The subnet1 index node has section edges to
its child documents.

````{query}
:view: depth0
:caption: Children of subnet1
id://belief-network-test-1-subnet-1 k-section-s(1)
````

## Parse Error Handling

A malformed query body must render a visible error block rather than silently
failing or leaving the sentinel in the output.

````{query}
:view: depth0
s-s-s
````

## Empty Result Set

A valid query that happens to return no results must render gracefully — an
empty table, not a panic or a raw sentinel.

````{query}
:view: edge_count
:caption: Empty — nothing with this schema
schema == _no_such_schema_xyz_
````
