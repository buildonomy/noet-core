---
bid = "10000000-0000-0000-0000-000000000099"
schema = "Document"
title = "Magic Continue Test Document"
---

# Magic Continue Test Root

This document tests the `{#__continue}` magic heading ID, which folds a heading
back into the preceding section node instead of creating a new one.

## Single Continue

First paragraph of the section.

## Single Continue {#__continue}

Second paragraph — belongs to the same node as "Single Continue".

## Independent Section

This heading has a distinct title and no `{#__continue}` annotation, so it
becomes its own section node. Confirms that the continue only applies to the
immediately preceding node.

## First Of Chain

Opening paragraph of a multi-continue chain.

## First Of Chain {#__continue}

Second paragraph of the chain.

## First Of Chain {#__continue}

Third paragraph — still folded into "First Of Chain".

## After Chain

A fresh section after the chain ends. Should be its own node.

## Section With Explicit Anchor {#explicit-anchor}

This heading has a real explicit anchor (not `__continue`). It must create its
own section node, and `{#explicit-anchor}` must be the node's id.

## Explicit Anchor {#__continue}

Content that continues "Section With Explicit Anchor". The prior node's explicit
anchor must be preserved — the continue heading must not clobber it.