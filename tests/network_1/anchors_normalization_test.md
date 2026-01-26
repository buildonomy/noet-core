---
bid = "50000000-0000-0000-0000-000000000001"
schema = "Document"
---

# Anchor Normalization Test

This document tests Issue 3: ID normalization and collision detection after normalization.

## API & Reference {#API & Reference}

Explicit anchor with special characters: `{#API & Reference}`
Should normalize to: `api--reference` for collision check.

## Section One {#Section One!}

Explicit anchor with space and punctuation: `{#Section One!}`
Should normalize to: `section-one` for collision check.

## My-Custom-ID {#My-Custom-ID}

Explicit anchor with mixed case and hyphens.
Should normalize to: `my-custom-id` for collision check.

## Configuration

Title-derived ID with no special chars.
Should get: `configuration`.

Expected behavior:
1. All explicit anchors are normalized via `to_anchor()` before collision check
2. Prevents HTML anchor conflicts from case/punctuation differences
3. Original anchor syntax is preserved in markdown
4. Two headings with IDs that normalize to same value → second gets Bref
