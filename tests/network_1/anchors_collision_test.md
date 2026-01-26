---
bid = "30000000-0000-0000-0000-000000000001"
schema = "Document"
---

# Anchor Collision Test Document

This document tests Issue 3: collision detection and Bref fallback for duplicate titles.

## Details

First occurrence of "Details" heading.
Should get ID: `details` (title-derived, no anchor needed).

## Implementation

Unique title, should get ID: `implementation`.

## Details

Second occurrence of "Details" heading - COLLISION!
Should get ID: `<bref>` (Bref fallback injected as `{#<bref>}`).

Expected behavior:
1. First "Details" heading: No anchor injected (unique at parse time)
2. Second "Details" heading: Gets `{#<bref>}` injected for uniqueness
3. Both headings create distinct nodes with different IDs
4. Links can reference both: `#details` and `#<bref>`

## Testing

Another unique heading.
