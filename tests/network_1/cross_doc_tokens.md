---
bid = "10000000-0000-0000-0000-000000000021"
schema = "Document"
title = "Cross Doc Tokens"
---

# Cross Doc Tokens

This document has a nested heading structure that reproduces the collision pattern
from the Issue 47 bug: two sibling h3 sections under one h2, where the second sibling
(sort_key=1) collides with a stale cross-document Section edge injected from
`cross_doc_notation.md`'s `session_bb` neighbourhood.

## Literals

### Examples

The first h3 sibling under `## Literals` (Section sort_key=0).
Children of this section must survive Phase 2 intact.

#### Characters and Strings

An h4 child of Examples.

#### Numbers

Another h4 child of Examples.

### Character and String Literals

The **second** h3 sibling under `## Literals` (Section sort_key=1).
This is the section that was incorrectly removed in the bug: a stale Section edge
from `notation.md#string-table-productions` with sort_key=1 pointing to this
document's root would be injected into `doc_bb`, colliding with this section at
order `[81, 0, 1]`, then swept out when the Epistemic edge for the notation section
was later processed without a Section weight.

#### Character Literals

An h4 child of Character and String Literals.

#### String Literals

Another h4 child — also removed by the bug due to order-prefix sweep.

#### Character Escapes

Third h4 child — also removed by the bug.

### Byte and Byte String Literals

The third h3 sibling (sort_key=2). Must survive Phase 2 intact — used as a
witness that only the second sibling's subtree was incorrectly removed.

#### Byte Literals

An h4 child of Byte and Byte String Literals.