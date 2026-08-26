---
title = "User Experience Depth Model"
authors = "Andrew Lyjak"
last_updated = "2026-07-08"
status = "Draft"
version = "0.1"
---

# User Experience Depth Model

How noet delivers value at each level of user investment, and where the
current product fails to do so.

## 1. The Governing Principle

noet's philosophical stance is **smooth iterative deepening** (see
[`smooth_iterative_deepening.md`](../design/smooth_iterative_deepening.md)): a user's first
experience should be simple, and as they go deeper they should be able to take
localized control without disturbing what already works. Most tools fail this
because either the initial experience is hard or the step from "simple" to "in
control" requires scaling a cliff of background knowledge.

This document translates that principle into concrete, testable UX constraints for
noet. It defines **depth levels**, specifies what the user should experience at each
level, and identifies where the current product violates the model.

## 2. Depth Levels

Each level describes a stage of user investment. The key invariant: **value must be
felt before the next concept is introduced.** A user at depth N should never need to
understand concepts from depth N+2 to get their work done.

### Level 0: Viewer (zero authoring)

**Who**: Someone handed a link to a rendered noet site. A reviewer, auditor, or
teammate browsing compiled output.

**What they do**: Navigate, search, read, follow cross-references, inspect
traceability tables.

**Concepts required**: None. It's a website.

**Value delivered**: Interconnected documentation with search and structural
navigation. Cross-domain traceability visible without switching between tools.

**Current state**: Partially working. The core navigation, search, and page
rendering function, but the viewer has significant UX problems at this level. ⚠️

**Viewer-specific violations at Level 0:**

1. **Metadata drawer speaks implementation language.** The relationship panel
   displays edges colored and labeled by WeightKind (Section / Epistemic /
   Pragmatic). These terms mean nothing to a reviewer. The drawer is
   load-bearing — it's the primary way to discover what a node is connected
   to — but it requires Level 4-5 vocabulary to interpret. At Level 0, the
   viewer should show relationships in terms the reader can act on: "linked
   documents," "parent sections," "coverage claims" — not edge type taxonomy.

2. **Navigation overwhelm on large corpora.** The navigation tree renders the
   full network hierarchy. For a deeply nested corpus with long titles (common
   in engineering documentation), the nav drawer on a typical laptop screen
   competes with or obscures the page content. There is no mechanism to scope
   the navigation to a relevant subset — the viewer always shows everything.

3. **No audience-specific entry points.** Today every viewer session starts at
   the same root and loads the same corpus. But different readers need different
   slices: a software engineer wants design docs and linked requirements; an
   auditor wants compliance matrices and hazard reports; a systems engineer
   wants architecture and interface specs. The viewer has no concept of
   "start here, with these networks loaded" — which means a 50-network corpus
   presents the same undifferentiated wall to everyone.

   The solution is not viewer infrastructure — it's **authored landing pages**.
   The Organizer (Level 2) creates audience-specific pages that use `{query}`
   directives to surface the relevant slice of the graph:

   ```markdown
   # Flight Software Overview

   Key design documents and their linked requirements.

   ```{query}
   id://fsw-design composed_of(*)
   ```

   ## Coverage Status

   ```{query}
   id://fsw-requirements traces_to(*)
   ```
   ```

   Each landing page is a normal document in the corpus — no special viewer
   support needed. The Organizer links to these pages from the network
   index or a top-level navigation page. Different audiences get different
   starting URLs that lead to curated content.

   This approach has several advantages over a viewer-side "role" abstraction:

   - **No viewer changes**: landing pages are just documents with queries.
   - **No manifest duplication**: the viewer loads the same `manifest.json`
     and `global.msgpack` regardless of audience. The global manifest is a
     fixed cost that cannot be avoided without per-audience builds.
   - **Content is visible and editable**: landing pages are Markdown files
     that the Organizer writes and reviews, not opaque JSON config.
   - **Composable with versions**: each version's build includes its own
     landing pages. No version × role matrix to manage.
   - **Incrementally authorable**: start with one landing page for one
     audience. Add more as needed. No upfront infrastructure.

   The key insight: audience-specific entry points are a **Level 2 authoring
   concern** (the Organizer writes landing pages) that solves a **Level 0
   consumption problem** (the Viewer gets a focused starting point). The
   existing query system provides all the infrastructure needed.

### Level 1: Author (single documents)

**Who**: Someone writing or editing documents that will be compiled by noet. They
may not have set up the project — someone else ran `noet init` and configured the
networks.

**What they do**: Write Markdown files with frontmatter. Add cross-references
using `[[wiki-links]]` or standard Markdown links. Run `noet parse` to see their
changes in the viewer.

**Concepts required**:
- Markdown with frontmatter (YAML or TOML — not both, pick one per project)
- `[[link-target]]` syntax for cross-references
- `noet parse --html-output _site .` to compile

**Concepts they should NOT need**:
- BID/Bref (system-managed, appears in frontmatter after first compile but
  should be ignorable)
- Section/Epistemic/Pragmatic edge types
- WeightKind, NodeKey, DocCodec
- Network configuration (whitelist/blacklist)
- `{network_children}` directive (should be in the `index.md` they didn't write)

**Value delivered**: Their documents are automatically linked, searchable, and
navigable. Cross-references resolve and survive file renames. The compiled site
is always up to date.

**Current state**: Partially working. The cliff is that BIDs appear in frontmatter
immediately and look alarming. The `[[wiki-link]]` experience is functional but
the error messages for unresolved links are implementation-flavored (diagnostic
types, not human sentences). Authors must understand the `index.md` structure to
know why their file isn't appearing. ⚠️

### Level 2: Organizer (project structure)

**Who**: Someone setting up a noet project or reorganizing an existing one.
Creates networks, decides what goes where, configures filtering.

**What they do**: Run `noet init`, create `index.md` files, set up
whitelist/blacklist patterns, organize subnets.

**Concepts required**:
- Network = directory with `index.md`
- `id` field (required, unique, human-chosen)
- `title` field
- `{network_children}` directive (controls landing page listing)
- Whitelist/blacklist glob patterns (for scoping what's included)

**Concepts they should NOT need**:
- BID internals (auto-managed)
- Edge type taxonomy (Section/Epistemic/Pragmatic)
- Codec system
- The compilation pipeline model

**Value delivered**: They control the structure and scope of the compiled site.
They can create focused views (e.g., "only design docs" or "only active
requirements") without touching the underlying data model.

**Current state**: This works but the documentation is heavy. `network_authoring.md`
is a 300-line reference document. The `noet init` command exists and handles the
interactive flow. The gap is that there's no progressive tutorial — the user goes
from "nothing" to a full reference manual. ⚠️

### Level 3: Connector (cross-document relations)

**Who**: Someone creating traceability claims across documents. They want to
declare "this design document addresses these requirements" or "this test covers
this hazard control."

**What they do**: Use `{maps_to}` (and the forthcoming generalized relation
directives from Issue 71) to declare edges between documents they don't own.

**Concepts required**:
- Relation directives: `{maps_to}`, `{implements}`, `{verifies}`, etc.
- Source vs. sink directionality (which end is "parent"?)
- How to reference target documents (by ID, title, or link)
- How to inspect relations in the viewer (traceability tables)

**Concepts they should NOT need**:
- Owner semantics (the `@` role — important for the model, but the directive
  handles it implicitly)
- WeightKind enum values
- The pragmatic vs. epistemic distinction (most cross-document claims are
  pragmatic; the system should default correctly)
- Codec internals

**Value delivered**: Traceability relationships are explicit, queryable, and
visible in the viewer. Coverage gaps surface as missing edges. When a document
is updated, the user can see what depends on it.

**Current state**: `{maps_to}` works. Issue 71 (generalized relation directives)
is in progress. The gap: the user must understand source/sink directionality
to choose the right verb, and there's no inline feedback when they get it
backwards. The traceability table exists but doesn't flag gaps — you have to
visually scan for missing rows. ⚠️

### Level 4: Integrator (multi-format ingestion)

**Who**: Someone bringing non-Markdown data into the graph. Spreadsheets, YAML
configs, external API exports, source code artifacts.

**What they do**: Configure XLSX ingestion, write codec extensions, set up
build pipelines that transform external data into noet-parseable inputs.

**Concepts required**:
- The codec system (WALK_CODECS, CLAIM_MAP, DocCodec trait)
- IRNode structure
- The compilation pipeline (multi-pass, diagnostic-driven resolution)
- Build system integration (Makefiles, CI pipelines)

**Concepts they should NOT need**:
- GraphBuilder internals
- BeliefBase data structures
- Event streaming architecture
- Shard/search index internals

**Value delivered**: Heterogeneous data sources become nodes in the same graph.
An engineer can navigate from a spreadsheet row to a Markdown design doc to a
YAML config to a source file — all in one viewer.

**Current state**: This works for Markdown, TOML, and XLSX. A downstream
consumer project demonstrates custom codecs (CMake, YAML blackboard/algorithm/
enum, C++ headers). The BACKLOG documents multiple codec API footguns and
ergonomic issues. The codec tutorial doesn't exist — you learn by reading that
downstream project's source and the BACKLOG's warnings. ❌

### Level 5: Extender (platform development)

**Who**: Someone building on top of noet-core as a library. Writing new viewer
features, query backends, CI integrations, or entirely new applications.

**What they do**: Use the noet-core Rust API directly. Build custom binaries
(like a downstream C++ codec consumer). Extend the MCP server. Write custom
query expressions.

**Concepts required**: Everything. The full data model, compilation pipeline,
identity system, event architecture, query algebra.

**Value delivered**: noet-core is a platform, not just a tool. The extender
builds domain-specific applications on top of a general-purpose document graph.

**Current state**: This is where noet-core actually excels — the library API
is well-documented in design docs, the architecture is clean, the test suite
is extensive. The gap is that Level 5 capabilities leak into Levels 1-3. ⚠️

---

## 3. Current Violations

Where the product forces users to a depth level they shouldn't need.

### 3.1 BID injection into frontmatter (Level 5 concern at Level 1)

BID injection (`--write` flag) freezes system-generated UUIDs into source file
frontmatter. When used, a Level 1 author sees UUIDs appear in files they authored:

```/dev/null/example_frontmatter.md#L1-5
---
bid = "01923abc-def0-7fff-bcd1-6ace77cb4a7d"
id = "my-document"
title = "My Document"
---
```

However, the actual depth violation here is more nuanced than "BIDs in source
files." In practice, most application workflows already bypass `--write`:

- **Without `--write` or `--db`**: BIDs are ephemeral (regenerated each run via
  `Uuid::now_v7()`). Structure and relationships are stable; absolute BID values
  are not. This works for single-session workflows.
- **With `--db`**: BIDs persist in a SQLite cache across runs. Source files stay
  clean. Identity is stable as long as the cache exists.
- **With `--write`**: BIDs are frozen into source frontmatter. Identity is
  portable — it survives cache deletion, repository cloning, and independent
  corpus comparison.

The `--write` flag conflates two distinct operations: **source normalization**
(link path updates, title sync) and **BID injection** (freezing identity into
source). An author might want the former without the latter.

**The real question**: when is BID-in-source actually needed? The primary use
cases are:

1. **Cold storage / archival**: freeze a corpus's identity state so it can be
   reconstituted without a cache DB. Important for compliance snapshots.
2. **A/B corpus comparison**: compare two versions of the same corpus by BID
   identity. Requires stable BIDs across independently compiled instances.
3. **Content-addressed section identity** (Issue 36): migrating BIDs when
   sections move between documents. With content hashing, source-injected BIDs
   become less necessary — the content hash *is* the stable identity, and the
   BID can be derived from it.

For live authoring workflows, the cache DB or output shards provide sufficient
identity persistence without touching source files. This suggests `--write`
should be reframed as an archival/export operation, not the default workflow.

**Recommendation**: Split `--write` into two concerns:
- `--write` (or just default behavior): normalize links and paths in source
- `--freeze-bids` (or `--archive`): inject BIDs into source for cold storage

This makes the common case (Level 1 author running `noet parse`) clean — source
files are never modified with system plumbing — while preserving the archival
capability for Level 4-5 workflows that need portable identity.

### 3.2 Three frontmatter formats (unnecessary decision at Level 1)

noet auto-detects TOML, YAML, and JSON frontmatter. This sounds like flexibility
but is actually an unforced decision: the author must choose a format, and different
files in the same project may use different formats. This is a Level 5 flexibility
imposed on Level 1.

**Recommendation**: `noet init` should default to YAML frontmatter, since YAML
is the industry standard for Markdown frontmatter across renderers (Jekyll,
Hugo, Docusaurus, Obsidian, pandoc, MDX). Defaulting to TOML would prioritize
noet's internal preference over cross-tool compatibility — a Level 5 choice
imposed on Level 1 authors. Auto-detection remains for ingesting existing
corpora in any format, but the *authoring* path should match what authors
already know from other tools.

### 3.3 Vocabulary leak (Level 4-5 terms at Level 1-2)

Diagnostic messages, documentation, and error output use implementation vocabulary:
BeliefBase, BeliefNetwork, DocCodec, WeightKind, IRNode, ParseDiagnostic. A Level 1
author encountering `ParseDiagnostic::UnresolvedReference` in compiler output gets
no useful signal — they need "Link to 'foo' could not be resolved. Did you mean
'bar'?"

**Recommendation**: User-facing messages (CLI output, diagnostics, documentation
landing pages) should use plain language. Implementation terms belong in API docs
and design documents, not in `noet parse` output.

### 3.4 Edge type selection (Level 5 concern at Level 3)

When a Level 3 user writes a `{maps_to}` directive, they're creating a pragmatic
edge. They shouldn't need to know that. The directive should default to the
appropriate edge type. Only a Level 5 extender building custom semantics needs to
choose between Section, Epistemic, and Pragmatic explicitly.

**Recommendation**: Relation directives should have a sensible default edge type.
The three-type taxonomy is a powerful modeling tool — but it's a modeling tool, not
a user-facing decision. Expose it only in the API and advanced configuration.

### 3.5 `noet parse` requires `index.md` before producing value (Level 2 prerequisite at Level 1)

Today, `noet parse some-directory/` fails or produces nothing useful if the
directory has no `index.md`. The user must run `noet init` first, which
requires choosing an `id`, understanding what a network is, and deciding
whether to include a `{network_children}` marker. This is a Level 2
prerequisite imposed on the Level 1 experience.

The smooth iterative deepening principle says the user should feel value
before learning the next concept. Today: value requires `noet init` →
understanding `id` → understanding `{network_children}`. What if it didn't?

**Recommendation**: `noet parse` should auto-generate a default `index.md`
in memory (not written to disk) when none exists. The `id` derives from the
directory name, `title` derives from `id`, `{network_children}` is included.
The user gets a working site from `noet parse --html-output _site .` with
zero configuration. When they need to customize — change the title, add
filtering, organize subnets — *then* they learn about `index.md` and
`noet init`. This collapses the Level 1→2 prerequisite: configuration is
deferred until the user needs it.

### 3.5a No "first five minutes" documentation path (missing Level 1 onramp)

Today's entry point is the README, which opens with a Rust API example. The CLI
entry point (`noet init`) exists but isn't documented in a tutorial. There is no
"create a project, add two documents, link them, compile, see the result" walkthrough.

**Recommendation**: The README and/or a top-level `GETTING_STARTED.md` should have
a command-line walkthrough that gets from zero to a rendered, linked site in under
5 minutes. No Rust code. No data model explanation. Just: create files, compile,
open. With the zero-config `noet parse` from §3.5, even `noet init` can be
omitted from the first tutorial.

### 3.6 No progressive documentation path (cliff between Level 2 and Level 3)

The documentation jumps from `network_authoring.md` (Level 2 reference) directly to
`beliefbase_architecture.md` (Level 5 specification). A Level 3 user who wants to
add traceability claims has no guide — they must read the design spec to understand
`{maps_to}`.

**Recommendation**: A `traceability_guide.md` or similar that covers relation
directives, source/sink semantics, and traceability table interpretation — without
requiring the user to understand the compilation pipeline, BID internals, or the
codec system.

### 3.7 Metadata drawer uses implementation vocabulary (Level 4-5 at Level 0)

The viewer's metadata panel labels relationships by `WeightKind` — "Section,"
"Epistemic," "Pragmatic" — with corresponding color coding. A Level 0 viewer
(reviewer, auditor, teammate) has no framework for interpreting these categories.
The information is genuinely useful (different edge types carry different meaning)
but the labels are implementation jargon.

**Recommendation**: The viewer should display relationship categories in
user-facing terms. Possible mappings:

| Internal term | User-facing alternative | Rationale |
|--------------|------------------------|-----------|
| Section | Structure / Contains | Describes containment hierarchy |
| Epistemic | References / Cites | Describes knowledge dependencies |
| Pragmatic | Traces to / Covers | Describes actionable claims |

The exact terms depend on context. The internal model stays the same — only
the display label changes.

### 3.8 No navigation scoping for large corpora (Level 2 concern at Level 0)

The viewer's navigation tree renders the complete network hierarchy, regardless of
corpus size. For a corpus with 50+ networks, deeply nested subnets, and long
engineering titles, the nav drawer on a 13–15″ laptop screen competes with page
content for horizontal space. The user cannot collapse irrelevant branches from the
start — they must manually explore and close subtrees.

The sharding infrastructure already supports lazy loading of individual networks.
What's missing is a way for the **Organizer** (Level 2) to define scoped views
that the **Viewer** (Level 0) benefits from without configuration.

**Recommendation**: Improve the **nav tree UX** for large corpora. Two
complementary approaches:

1. **Authored landing pages** (see §2, Level 0 item 3): the Organizer writes
   audience-specific pages with curated `{query}` directives that surface
   relevant subsets of the graph. These act as entry points — different
   audiences bookmark different landing pages. No viewer infrastructure
   needed beyond what already exists.

2. **Nav tree collapsibility improvements**: the viewer should start with
   top-level networks collapsed and expand subtrees on demand. Combined
   with the existing shard lazy-loading, this reduces visual noise without
   hiding data. Deep-link URLs already expand the relevant subtree
   automatically.

### 3.9 View-to-edit cliff (broken bridge between Level 0 and Level 1)

The deepest violation of smooth iterative deepening in the entire product:
**viewing and editing live in completely different systems with no shared
state.** The viewer knows the graph but can't modify source. The text editor
can modify source but doesn't know the graph.

Today's editing workflow:

```/dev/null/edit_workflow.txt#L1-6
1. `noet watch --html-output _site --serve .`  → start local dev server
2. Open browser, navigate to a document, read it
3. Want to edit? Switch to text editor
4. Mentally map the viewer URL path → filesystem path → find the file
5. Edit the Markdown source
6. Watch detects change, recompiles, browser reloads
```

Steps 3-4 are the cliff. The user leaves the rendered, graph-aware view and
enters a raw source editor that has no awareness of the document's identity,
relationships, or diagnostic state. A Level 0 viewer who spots a typo can't
fix it. A Level 1 author must context-switch between two applications. A
Level 3 connector who wants to add a `{maps_to}` directive has to know the
source file path and the directive syntax.

Three infrastructure pieces address this at different levels:

1. **LSP (Issue 11)** makes the *text editor* graph-aware. Diagnostics appear
   inline. Hover shows node metadata. Autocomplete resolves `[[wiki-links]]`.
   This bridges Level 1 (authoring) with the graph — the editor becomes a
   first-class noet client, not a dumb text tool.

2. **`noet watch` as a real server** (not just a static file server with
   live-reload injection). Today `watch` uses a minimal `DevServer` that
   serves files from the HTML output directory and injects a WebSocket reload
   script. For the editing bridge to work, `watch` needs to be a real
   application server that can:
   - Expose the compiled graph via a local API (same query surface as MCP)
   - Accept source edits from the browser and write them back to disk
   - Push incremental graph updates to the viewer on recompile
   - Serve the LSP protocol to editors on the same machine

   The `watch` command already owns the compilation loop and the
   `WatchService` manages the `BeliefBase` cache. The missing piece is
   exposing this state to both the browser and the editor as a unified
   service, rather than writing static files and serving them dumbly.

3. **Attestation fabric** (`attestation_fabric.md`) bridges the *reader-to-
   author* feedback loop for shared/deployed sites. When a reviewer leaves
   a comment or flag on a deployed (static) site, that feedback is an
   attestation event anchored to a specific `(site_url, asset_version, bid)`.
   The author should see these attestations surfaced in their editing
   environment — either in the browser during a `watch` session or in their
   IDE via LSP. This turns the collaboration overlay from a read-only
   annotation layer into a feedback channel that drives authoring decisions.

   The attestation fabric is the mechanism; the UX implication is that the
   author's workspace should show "3 unresolved comments on this node" and
   "sign-off required from $ROLE" alongside the normal diagnostic output.

**Recommendation**: The editing experience should be a design priority, not
an afterthought. The depth model should target:

- **Level 0→1 bridge**: A "suggest edit" or "view source" action in the
  viewer that opens the corresponding source file in the local editor
  (during `watch` sessions) or proposes a redline (on deployed sites).
- **Level 1 experience**: The text editor is graph-aware via LSP.
  Diagnostics, hover, autocomplete, go-to-definition — all operating
  on the compiled graph, not just syntax.
- **Level 0 feedback**: Attestation events (comments, flags, sign-offs)
  from the collaboration overlay are visible to the author in their
  editing environment, creating a closed feedback loop.

Issue 11 (Basic LSP) and the `watch`-as-server evolution are the critical
path items. The attestation integration is a later layer that builds on
both.

---

## 4. The Depth Test

A design constraint for evaluating changes to noet:

> **For any new feature, identify its depth level. If the feature's user-facing
> surface requires concepts from a deeper level, the design has a leak.**

Examples:
- A new directive that requires understanding WeightKind → leak (Level 5 at Level 3)
- A CLI flag that only matters for codec development → fine if hidden from `--help`
  at the default verbosity
- A diagnostic message that names internal types → leak (Level 4+ at Level 1)
- A frontmatter field that's system-managed appearing in user-authored files →
  leak (Level 5 at Level 1)

The test is not "can we hide it?" — it's "does the user at level N need to
encounter this to get value at level N?" If not, it's a leak, and the fix is either
deferral (don't show it until the user reaches the relevant depth) or encapsulation
(handle it internally without surfacing it).

---

## 5. Subtraction Candidates

Features and behaviors to consider removing from shallower depth levels. These are
not decisions — they are candidates for evaluation.

| Current behavior | Depth violation | Candidate fix |
|-----------------|----------------|---------------|
| `--write` conflates link normalization + BID injection | L5 at L1 | Split into `--write` (links only) + `--freeze-bids` (archival) |
| Three frontmatter formats | L5 at L1 | Default to one; auto-detect for ingestion |
| `BeliefBase`/`BeliefNetwork` in CLI output | L5 at L1 | Plain-language messages |
| `WeightKind` in relation directives | L5 at L3 | Sensible defaults per directive |
| `noet parse` requires `index.md` | L2 at L1 | Auto-generate default network in memory when absent |
| `{network_children}` required knowledge | L2 at L1 | Auto-generate in `noet init` |
| Diagnostic type names in output | L4 at L1 | Human-readable error messages |
| Edge labels in viewer (Section/Epistemic/Pragmatic) | L4-5 at L0 | User-facing labels (Structure/References/Traces to) |
| Full nav tree on large corpora | L2 at L0 | Collapsed-by-default nav + authored landing pages |
| No audience-specific entry points | Missing L0 | Authored landing pages with `{query}` directives |
| No getting-started tutorial | Missing L1 | Create one |
| No traceability guide | Missing L3 | Create one |
| View-to-edit cliff (no shared state) | Missing L0↔L1 bridge | LSP + `watch`-as-server + viewer "edit" action |
| Reader feedback invisible to authors | Missing L0→L1 channel | Attestation events surfaced in editor via LSP |

---

## 6. What This Document Is Not

This document does not specify implementation changes. It identifies UX constraints
that should inform implementation decisions. Each subtraction candidate that is
approved becomes an issue or a modification to an existing issue.

This document also does not define the philosophical rationale for smooth iterative
deepening. That lives in [`smooth_iterative_deepening.md`](../design/smooth_iterative_deepening.md).
This document is the *engineering application* of that philosophy to noet's product
surface.

---

## 7. References

- [`smooth_iterative_deepening.md`](../design/smooth_iterative_deepening.md) — Philosophical stance
- [`network_authoring.md`](../design/network_authoring.md) — Level 2 reference (network setup)
- [`beliefbase_architecture.md`](../design/beliefbase_architecture.md) — Level 5 specification
- [`dag_model.md`](../design/dag_model.md) — Conceptual introduction to the graph model
- [`myst_directive_architecture.md`](../design/myst_directive_architecture.md) — Directive pipeline
- [`collaboration_overlay.md`](../design/collaboration_overlay.md) — Attestation annotation layer for static sites
- [`attestation_fabric.md`](../design/attestation_fabric.md) — General provenance and annotation infrastructure
- Issue 11 — Basic LSP implementation (editing bridge)
- Issue 36 — Content-based section identity (BID migration)
- Issue 71 — Generalized relation block directives
- Issue 73 — Versioned rendering (multi-version SPA viewer)
- BACKLOG — Codec API footguns and ergonomic issues
