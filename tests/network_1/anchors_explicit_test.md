---
bid = "40000000-0000-0000-0000-000000000001"
schema = "Document"
---

# Explicit Anchor Preservation Test

This document tests Issue 3: explicit anchors are preserved even when titles change.

## Getting Started {#getting-started}

This heading has an explicit anchor.
The anchor should be preserved even if we rename the title later.

## Setup {#custom-setup-id}

This heading has a custom explicit anchor.
Should be preserved exactly as written (normalized for collision check).

## Configuration

This heading has NO explicit anchor.
Should get title-derived ID: `configuration`.
If title changes, ID should auto-update.

## Advanced Usage {#usage}

Explicit anchor that's different from title-derived slug.
Title would slug to `advanced-usage`, but anchor is `usage`.
The explicit anchor wins.

Expected behavior:
1. Explicit anchors are preserved in markdown: `{#getting-started}`, `{#custom-setup-id}`, `{#usage}`
2. Headings without anchors don't get any injected (unless collision)
3. Title changes on headings WITHOUT anchors → ID updates automatically
4. Title changes on headings WITH anchors → anchor preserved, ID doesn't change
