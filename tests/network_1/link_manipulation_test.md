---
bid = "10000000-0000-0000-0000-000000000001"
title = "Link Manipulation Test"
---

# Link Manipulation Test

This document tests link transformation to canonical format with Bref in title attribute.

## Simple Links

Basic link to another document:
[Simple Link](./file1.md)

Link with anchor:
[Link with Anchor](./file1.md#section-a)

Same-document anchor:
[Same Doc Anchor](#explicit-brefs)

## Explicit Brefs

User-provided Bref in title attribute:
[Custom Text](./file1.md "bref://abc123456789")

Bref with auto_title enabled:
[Auto Title](./file1.md "bref://abc123456789 {\"auto_title\":true}")

Bref with user words:
[Link Text](./file1.md "bref://abc123456789 Custom annotation here")

Full format:
[Link](./file1.md "bref://abc123456789 {\"auto_title\":true} See this section")

## Nested Directory Links

Link to file in subdirectory:
[Subnet File](./subnet1/subnet1_file1.md)

Link from root to nested with anchor:
[Nested Section](./net1_dir1/hsml.md#definition)

## Section Headings {#explicit-brefs}

These headings will have their own Brefs and can be linked to.

### Subsection One {#subsection-one}

Content here.

### Subsection Two

This will get an auto-generated anchor.

## Multiple Links to Same Target

First link:
[First Reference](./file1.md)

Second link with different text:
[Second Reference](./file1.md)

Third link with manual Bref:
[Third Reference](./file1.md "bref://def456789012")

## Expected Transformations

After inject_context, all links should be transformed to:
`[text](relative/path.md#anchor "bref://abc123...")`

Links with matching text should get auto_title enabled.
Links with custom text should preserve user's choice.
Brefs should be stable even if files move.
