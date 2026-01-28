---
bid = "10000000-0000-0000-0000-000000000002"
schema = "Document"



[sections."id://background"]
schema = "Section"
complexity = "medium"
priority = 2

[sections."id://api-reference"]
schema = "Section"
complexity = "low"
priority = 3

[sections.unmatched]
schema = "Section"
complexity = "critical"
note = "This section has no matching heading - should be logged as info"
---

# Sections Test Document

This document tests the sections metadata enrichment feature (Issue 02).

## Introduction

This section has no sections entry, so it gets default metadata only.

BID matching is possible by including a sections entry like:
`[sections."bid://12345678-90ab-cdef-1234-567890abcdef"]`

However, heading BIDs are auto-generated during parsing, so we can't predict them
in a static test fixture. BID matching is better tested with dynamic tests that
capture the actual BID after parsing.

This test focuses on ID (anchor) and Title matching, which we CAN control.

## Background {#background}

This section matches by ID anchor (high priority after BID).

The markdown has: `{#background}` anchor
The frontmatter has: `sections."id://background"`
Issue 03 parses the anchor and stores it in the node's `id` field.
Issue 02 matches via NodeKey::Id { net, id: "background" }.
This should be enriched with `complexity = "medium"` and `priority = 2`.

## API Reference

This section matches by title normalization (same priority as ID when no explicit anchor).

The title "API Reference" normalizes via to_anchor() to "api-reference".
The frontmatter has: `sections."id://api-reference"`
Issue 03 auto-generates the ID from the title (no explicit anchor).
Issue 02 matches via NodeKey::Id { net, id: "api-reference" }.
This should be enriched with `complexity = "low"` and `priority = 3`.

## Untracked Section

This heading has NO entry in the sections frontmatter initially.

During parsing, it will:
1. Create a node (markdown structure defines which nodes exist)
2. Get an auto-generated ID: `untracked-section` (via to_anchor on title)
3. Have an entry ADDED to the sections table with its auto-generated ID
4. Use default metadata initially (no custom fields from frontmatter)

This demonstrates that ALL markdown headings get nodes AND sections entries,
even if not pre-defined in frontmatter.

> Note: The "unmatched" section in frontmatter has no corresponding heading,
> so it should be garbage collected during finalize() (heading was removed from markdown).