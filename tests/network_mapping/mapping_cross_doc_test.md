---
bid = "10000000-0000-0000-0000-000000000033"
schema = "Document"
title = "Cross-Document Mapping Test"
---

# Cross-Document Mapping Test

This document exercises the `{maps_to}` directive when the owning section is in
a **different document** from the source and sink nodes.

The source and sink nodes are declared in `mapping_cross_doc_endpoints.md`:

- `id://xdoc-req-alpha` → "XDoc Requirement Alpha"
- `id://xdoc-req-beta`  → "XDoc Requirement Beta"
- `id://xdoc-impl-one`  → "XDoc Implementation One"

This tests the `compute_diff` initial edge filter fix (Bug 3 from Issue 61):
the filter previously required source or sink to be in `parsed_content`, which
excluded all cross-document mapping edges. The fix adds a third condition: the
edge passes if its `WEIGHT_OWNED_BY` bref resolves to a BID that IS in
`parsed_content`.

## XDoc Trace Mapping

This section owns the cross-document mapping edges. It is neither a source nor
a sink endpoint — it merely declares the relationships between nodes that live
in the sibling document `mapping_cross_doc_endpoints.md`.

````{maps_to} Pragmatic
source = ["id://xdoc-req-alpha", "id://xdoc-req-beta"]
sink = "id://xdoc-impl-one"
````
