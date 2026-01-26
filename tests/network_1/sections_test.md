---
bid = "10000000-0000-0000-0000-000000000001"
schema = "Document"

[sections."bid://20000000-0000-0000-0000-000000000002"]
schema = "Section"
complexity = "high"
priority = 1

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

## Introduction {#bid://20000000-0000-0000-0000-000000000002}

This section should match by BID (highest priority).

The frontmatter has: `sections."bid://20000000-0000-0000-0000-000000000002"`
This heading has the BID anchor `{#bid://20000000-0000-0000-0000-000000000002}` which should match.
This should be enriched with `complexity = "high"` and `priority = 1`.

## Background {#background}

This section should match by anchor (medium priority).

The frontmatter has: `sections."id://background"`
The anchor `{#background}` should match to the NodeKey::Id("background").
This should be enriched with `complexity = "medium"` and `priority = 2`.

## API Reference

This section should match by title anchor (lowest priority).

The frontmatter has: `sections."id://api-reference"`
The title "API Reference" → to_anchor("API Reference") = "api-reference" should match.
Since sections are not guaranteed unique titles, we use NodeKey::Id for matching.
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