---
bid = "10000000-0000-0000-0000-000000000034"
schema = "Document"
title = "Cross-Document Mapping Endpoints"
---

# Cross-Document Mapping Endpoints

This document declares the source and sink nodes referenced by `{maps_to}`
directives in `mapping_cross_doc_test.md`. It exists solely to exercise the
cross-document mapping scenario — the owning section lives in a different file.

## XDoc Requirement Alpha {#xdoc-req-alpha}

A requirement node declared in this document. Referenced as `id://xdoc-req-alpha`
by the `{maps_to}` directive in `mapping_cross_doc_test.md`.

## XDoc Requirement Beta {#xdoc-req-beta}

A second requirement node. Together with XDoc Requirement Alpha, forms the
`sources` side of the Cartesian product in the cross-document `{maps_to}` test.

## XDoc Implementation One {#xdoc-impl-one}

An implementation node. This is the `sink` side of the cross-document mapping.
After compilation, two Pragmatic edges should exist in `global_bb`:

- `xdoc-req-alpha → xdoc-impl-one`
- `xdoc-req-beta  → xdoc-impl-one`

Both owned by the bref of the "XDoc Trace Mapping" section in
`mapping_cross_doc_test.md`.