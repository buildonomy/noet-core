# Trade Study: Quasi-Dynamic Content Placement in Network Index Pages

**Version**: 0.1
**Status**: Decision Recorded
**Related Issue**: Issue 52 (Network Index Content Merge)

## Summary

`NetworkCodec::generate_deferred_html` needs to know (a) whether to inject an
auto-generated child listing into a network's `index.md` output and (b) where
to place it relative to authored prose.  The current implementation uses a raw
HTML comment `<!-- network-children -->` as a placement marker in the Markdown
body.  Before this convention hardens, we should evaluate the design space:
the choice determines the ergonomics of authoring network index pages, the
cleanliness of the separation between machine-generated and human-written
content, and the generalisability of the mechanism to other quasi-dynamic
content (backlink lists, tag clouds, etc.).

## Problem Statement

A belief network's `index.md` has two kinds of content:

1. **Authored prose** — written by a human, lives in `index.md`, should round-trip
   cleanly through `generate_source`.
2. **Machine listing** — generated at render time from graph relationships, must
   not be written back to source, changes whenever the graph changes.

The question is: **how does the author communicate placement intent to the
render pipeline**, and where does that intent live?

### Constraints

- Must not corrupt `generate_source` round-trips (no machine content in source)
- Must be readable without a running noet pipeline (plain Markdown viewers,
  GitHub, editors)
- Must be writable by humans without special tooling
- Should generalise: `network-children` is the first use case; backlinks, tag
  clouds, query results, etc. are plausible future uses
- Must integrate with `BeliefNode` / `payload` / `schema` architecture without
  fighting it

---

## Options

### Option A — Raw HTML Comment in Markdown Body (current)

```markdown
# My Network

Some introductory prose.

<!-- network-children -->

Notes that appear after the listing.
```

The `<!-- network-children -->` line is parsed by pulldown-cmark as an
`MdEvent::Html` block.  `NetworkCodec::generate_html` scans for it and
replaces it with a sentinel string.  `generate_deferred_html` replaces the
sentinel with the generated listing HTML.

**Pros**
- Zero parsing cost — pulldown-cmark already handles it
- Author controls placement exactly (before, between, after prose blocks)
- No schema registration required
- Invisible to readers in most Markdown renderers (HTML comments are hidden)
- Trivially extensible: `<!-- network-backlinks -->`, `<!-- query:tag=foo -->`

**Cons**
- HTML comment syntax is not author-ergonomic; authors may not know it exists
- Naïve renderers (GitHub, VS Code preview) hide it silently — no visual cue
  that "something will appear here"
- Looks like RST/Sphinx directives, which is a large conceptual surface area if
  generalised
- The comment string is a magic value with no schema backing — hard to discover
  and easy to mistype
- Opt-in by presence: forgetting the marker appends the listing at the end,
  which may surprise authors

---

### Option B — Frontmatter Payload Key

```toml
---
id = "my-network"
title = "My Network"
children_placement = "after_intro"   # or "top" | "bottom" | "none"
---

# My Network

Some introductory prose.
```

The `children_placement` key in frontmatter lands in `IRNode.document`, is
promoted to `BeliefNode.payload` via `TryFrom<&IRNode>`, and is readable from
`ctx.node.payload` in `generate_deferred_html`.  `generate_html` always
appends a sentinel; `generate_deferred_html` checks `payload` to decide
whether to replace it and — for values other than `"bottom"` — uses heuristics
(e.g. after the first heading, after the first paragraph) to find the splice
point in the rendered HTML.

**Pros**
- Configuration in frontmatter is idiomatic and author-visible
- Round-trip safe: frontmatter is source content, listing is never written back
- Machine-readable by any consumer of `BeliefNode` (LSP, SPA, API)
- No magic HTML comments; misspellings caught by schema validation (if
  registered)
- `children_placement = "none"` provides a clean opt-out

**Cons**
- Coarse positioning only: `"after_intro"` requires heuristics (what counts as
  "intro"?) that will be fragile and hard to define precisely
- Cannot express "between paragraph 2 and paragraph 3" without adding a
  positional index, which is brittle as prose changes
- Adds a well-known key to network nodes' payload with no schema registration
  yet — becomes informal convention unless a `network` schema is registered
- Does not generalise to arbitrary placement of multiple injection points in one
  document

---

### Option C — Schema-Registered Payload (full vision)

Register a `network` schema with `SCHEMAS` that formally defines `children_placement`
and other network-specific payload fields.  `NetworkCodec` checks
`ctx.node.schema == Some("network")` and reads typed, validated payload fields.

```toml
---
id = "my-network"
title = "My Network"
schema = "network"

[children_listing]
placement = "bottom"   # "top" | "bottom" | "none"
---
```

**Pros**
- Full architectural alignment with `BeliefNode.schema` / `payload` design
- Schema validation catches misspellings at parse time
- Payload fields are discoverable via schema registry
- Enables LSP autocompletion and hover docs on network frontmatter fields
- Clean foundation for other network-level configuration (custom sort order,
  listing depth, etc.)

**Cons**
- Requires implementing and registering a `network` schema — significant scope
  increase relative to Issue 52
- Still suffers from the coarse-positioning problem of Option B
- `schema = "network"` is implicit for any file named `index.md` — requiring
  authors to write it is redundant; auto-detecting it hides an important field
- Blocks Issue 52 on schema registry work (Issue 32)

---

### Option D — Named Heading as Anchor

Reserve a specific heading title as the injection point:

```markdown
# My Network

Some introductory prose.

## Contents

Notes after the listing.
```

If a heading with the title `"Contents"` (or `"Index"`, or configurable via
frontmatter) exists, the listing is injected immediately after that heading's
opening tag.  If absent, listing appends at the end.

**Pros**
- Pure Markdown — renders correctly in all viewers with a visible structural cue
- No hidden syntax; the heading is meaningful to human readers
- Round-trip safe: heading is authored content
- Natural: "Contents" is an expected section in an index page

**Cons**
- Magic heading title is implicit convention; hard to discover without docs
- Conflicts with legitimate authored sections named "Contents"
- Positioning is "after heading", not "at arbitrary point within prose" —
  less flexible than Option A
- Internationalisation: `"Contents"` doesn't work in non-English networks
  without configuration
- Does not generalise cleanly to multiple injection points or other content
  types

---

### Option E — Hybrid: Payload opt-in/out, HTML comment for position

```toml
---
id = "my-network"
title = "My Network"
children_listing = true   # opt-in; default true for network nodes
---

# My Network

Intro prose.

<!-- network-children -->

Footer prose.
```

`payload.children_listing = false` suppresses the listing entirely (no sentinel
emitted from `generate_html`).  When `true` (or absent, defaulting to true),
`generate_html` scans for the `<!-- network-children -->` HTML comment and uses
it as the position anchor; if not present, appends at end.

**Pros**
- Clean separation of concerns: payload controls opt-in/out, HTML comment
  controls position
- Opt-out is ergonomic (`children_listing = false` in frontmatter is readable)
- Positional control is available for authors who want it; ignorable for those
  who don't
- `children_listing` in payload is a simple boolean — no schema required, no
  heuristics needed
- Generalises: future `backlinks_listing`, `tag_cloud`, etc. follow the same
  pattern

**Cons**
- Two mechanisms for one feature (some cognitive overhead)
- HTML comment is still non-ergonomic for positional control
- Authors who want positional control still need to know the magic comment

---

## Decision

**Option A — Raw HTML comment in Markdown body.**

Chosen because it is already implemented, solves the immediate need, and
introduces no premature abstraction.  The generalisation question (payload keys,
schema registration, directive syntax) is deferred until there is a second
concrete use case that forces a design decision.

**Rationale for deferral:**

- Options B/C/E all require a `network` schema or well-known payload keys that
  don't exist yet.  Defining them now would be speculative design.
- Option D (named heading) creates naming conflicts and doesn't generalise.
- Option A is already working code.  Complexity should be added when the need
  is proven, not anticipated.
- When a second dynamic content type (backlinks, tag cloud, query results)
  arrives, the right generalisation will be visible.  Until then, YAGNI.

**Accepted drawbacks:**

- HTML comment is not author-ergonomic; misspellings produce silent no-ops.
- Magic string has no schema backing or LSP discoverability.
- If many directive types accumulate, this becomes a mini-directive language —
  at which point a new trade study is warranted.

### Revisit Triggers

Reopen this study if any of the following occur:

- A second quasi-dynamic content type needs placement control in Markdown
- Authors frequently mistype or misplace the marker (observed ergonomic failure)
- Issue 32 (schema registry) lands and a `network` schema is being defined

---

## Deferred Questions

These were identified during analysis but are not actionable until a revisit
trigger (above) fires.

- **Generalisation syntax**: if `<!-- backlinks -->`, `<!-- query:tag=foo -->`,
  etc. accumulate, the HTML comment approach becomes a mini-directive language
  (cf. RST).  Evaluate at that point.

- **Schema-backed opt-out**: a `children_listing = false` frontmatter key would
  be cleaner than removing the HTML comment marker.  Defer to Issue 32 /
  `network` schema registration.

- **LSP discoverability**: with no schema, the LSP cannot autocomplete or
  validate the marker string.  Acceptable until schema registry exists.