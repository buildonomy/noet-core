---
bid = "60000000-0000-0000-0000-000000000001"
schema = "Document"
title = "Inline Anchor Test Document"
---

# Inline Anchor Test Document

This document exercises inline anchors in non-heading blocks. Each anchor
marks a named node within a section without requiring a heading element.
Code spans like `{#not-an-anchor}` must not trigger node creation — only
bare anchor syntax in plain text position should be detected.

## Widget Requirements {#widget-requirements}

This section contains requirement-like nodes identified by inline anchors
rather than heading anchors. Each paragraph anchor should produce a child node
at depth `section.heading + 1` with the anchor ID as both its node ID and title.

{#req-001} **REQ-001** The widget shall accept inputs within the declared
operating range and reject out-of-range values with a documented error response.

{#req-002} **REQ-002** The widget shall produce deterministic outputs for
identical inputs under identical system state.

{#req-003} **REQ-003** The widget shall complete each operation within the
latency budget defined in the performance specification.

Plain paragraph with no anchor — should accumulate into the preceding anchor
node (req-003), not create a new node.

## Checklist Items {#checklist-items}

This section exercises inline anchors on list-item-style blocks. The anchor
identifies the item; the remainder is the item body.

{#chk-review} **Review:** All interface definitions have been reviewed by a
second engineer and any open questions are resolved.

{#chk-test} **Test:** Automated tests cover the nominal path and at least two
off-nominal paths for each public interface.

{#chk-doc} **Doc:** Public interfaces are documented with parameter types,
valid ranges, and error conditions.

## Code Block Guard {#code-block-guard}

A fenced code block containing `{#id}` syntax must not create a node. The
text inside is a raw `MdEvent::Text` event (unlike code spans which emit
`MdEvent::Code`), so the implementation must suppress anchor detection while
inside a plain fenced block.

```
{#not-an-anchor} This line looks like an inline anchor but is inside a
fenced code block and must be ignored by the parser.
```

No node with ID `not-an-anchor` should exist after parsing this document.

## Cross-Reference Target {#xref-target}

This section is a plain heading node. The inline-anchor nodes above can be
referenced from other documents the same way heading nodes are — by their ID.

A link to [req-001](inline_anchor_test.md#req-001) tests intra-document
reference resolution to an inline-anchor node.
