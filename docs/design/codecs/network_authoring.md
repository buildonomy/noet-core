---
title = "BeliefNetwork Authoring Reference"
authors = "Andrew Lyjak, Claude Code"
last_updated = "2025-04-28"
status = "Draft"
version = "0.1"
---

# BeliefNetwork Authoring Reference

## 1. What Is a BeliefNetwork?

A **BeliefNetwork** is a directory that noet treats as a named, structured scope for
documents. Any directory containing an `index.md` file is a BeliefNetwork. The
`index.md` defines the network's identity and governs which files it includes.

Networks nest: a subdirectory can be its own network (a **subnet**), forming a tree.
Each network owns its direct children — documents and subnets — and can filter them
with whitelist/blacklist glob patterns.

```
repo/
├── index.md              ← root network
├── docs/
│   └── guide.md          ← document owned by root network
├── requirements/
│   ├── index.md          ← subnet: "requirements"
│   └── req-001.md        ← document owned by requirements subnet
└── drafts/
    ├── index.md          ← subnet: "drafts"
    └── wip.md            ← document owned by drafts subnet
```

The root directory of a corpus must itself be a BeliefNetwork (contain an `index.md`).

---

## 2. The `index.md` File

`index.md` is the only required file in a BeliefNetwork. It has two parts:

1. **Frontmatter** — TOML, YAML, or JSON metadata block between `---` delimiters.
   Defines the network's identity and configuration. Format is auto-detected.
2. **Body** — Standard Markdown. Displayed as the network's landing page. May contain
   the `{network_children}` directive to render a listing of child documents.

```markdown
---
id = "my-network"
title = "My Network"
---

# My Network

A description of what this network contains.

````{network_children}
````
```

---

## 3. Frontmatter Fields

### 3.1 Required fields

#### `id`

**Type**: string  
**Required**: yes — the network will not compile without it  
**Constraints**: must be unique within the corpus; slug-style recommended (`kebab-case`)

The semantic identifier for this network. Used for cross-references, link resolution,
and graph identity. Unlike `bid`, `id` is human-authored and stable across machines.

```toml
id = "system-requirements"
```

### 3.2 Common optional fields

#### `title`

**Type**: string  
**Default**: derived from the `id` slug if absent

The display name for the network. Appears in the child listing of the parent network,
in search results, and in generated HTML.

```toml
title = "System Requirements"
```

#### `text`

**Type**: string (Markdown)

A short summary of the network's purpose. Indexed for search and displayed in the
network's metadata panel in the viewer.

```toml
text = "Top-level functional and performance requirements for the system."
```

#### `bid`

**Type**: UUID string (hyphenated)  
**Written by**: `noet compile --write` on first parse  
**Do not author by hand** unless migrating from another system

Stable unique identifier assigned by noet on first compile. Once present, the bid is
preserved across renames, moves, and content changes. Do not modify it.

```toml
bid = "01923abc-def0-7fff-bcd1-6ace77cb4a7d"
```

#### `schema`

**Type**: string (dotted schema name)

Associates the network with a schema definition. Used for structured metadata
validation and graph edge extraction. Most networks do not need this field.

```toml
schema = "myapp.component_network"
```

### 3.3 Child filtering fields

See §5 for full semantics and examples.

#### `whitelist`

**Type**: array of glob strings  
**Default**: `[]` (accept all)

Network-relative glob patterns. When non-empty, only files matching at least one
whitelist pattern are included as children of this network.

```toml
whitelist = ["docs/**/*.md", "specs/**/*.md"]
```

#### `blacklist`

**Type**: array of glob strings  
**Default**: `[]` (reject nothing)

Network-relative glob patterns. Files matching any blacklist pattern are excluded,
even if they also match a whitelist pattern.

```toml
blacklist = ["generated/**", "scratch/**", "*.draft.md"]
```

---

## 4. The Body: Markdown and `{network_children}`

The body of `index.md` is standard Markdown. It is parsed and stored as the `text`
payload of the network node (if `text` is not already set in frontmatter).

### 4.1 `{network_children}` directive

Place this MyST fenced directive anywhere in the body to render a live listing of the
network's child documents in the HTML output:

```markdown
`{network_children}`
```

For full semantics — ordering, HTML output structure, and the deferred generation
pass — see [`myst_directive_architecture.md` §6.1](./myst_directive_architecture.md#61-network_children).

### 4.2 Regular Markdown content

All standard Markdown is supported: headings, paragraphs, lists, links, code blocks.
Heading-level sections within `index.md` become child nodes of the network node in
the graph, exactly as in any other document.

Cross-references to other nodes use the standard link format:
```markdown
See [[other-network-id]] or [[req-001]] for details.
```

### 4.3 Naming nodes below the heading level

A heading becomes a node, and writing `{#id}` on it fixes that node's anchor instead
of deriving one from the title:

```markdown
## Fault Detection {#fdir}
```

The same `{#id}` syntax in a **paragraph or list item** makes that block its own node,
named and addressable without a heading of its own:

```markdown
## Requirements

{#req-001} The system shall detect loss of signal within 500 ms.

{#req-002} The system shall enter safe mode on detection.

- {#chk-a} Verify by test
- {#chk-b} Verify by analysis
```

That yields four sibling nodes under `Requirements`, each linkable as `[[req-001]]`
and each a valid `{maps_to}` target. Use it when items need to be addressed
individually but a heading apiece would bury the document in structure.

Things worth knowing before you rely on it:

- **Consecutive anchors are siblings**, not nested. Each sits one level below the
  heading that encloses it.
- **Text after an anchored block folds into it**, just as a paragraph below a heading
  belongs to that heading. The node ends at the next heading or next anchor.
- **The anchor may sit anywhere in the block**: `*Shall* {#req-003} hold.` works.
- **Ids are slugified, titles are not.** `{#Req_001}` gives id `req_001`, title
  `Req_001`. Prefer writing ids already slugified.
- **Duplicates are suffixed and warned about.** A second `{#req-001}` becomes
  `req-001-2`. Note this appends to the whole id, so a repeated `{#swdp-63}` becomes
  `swdp-63-2` rather than `swdp-64`.
- **Not detected in table cells.** `| {#id} x |` leaves the braces visible in the
  rendered output and creates no node. Code spans and fenced blocks are correctly
  ignored.

Full semantics, including HTML rendering and node-boundary effects, are in
[`myst_directive_architecture.md` §6.5](<./myst_directive_architecture.md#6.5-inline-anchor-nodes-(parse-only)>).
To fold a heading *into* the preceding node rather than create a new one, see
[§6.4, `{#__continue}`](<./myst_directive_architecture.md#6.4-__continue-heading-continuation-(parse-only)>).

---

## 5. Child Filtering: Whitelist and Blacklist

By default, a network includes all files in its directory (and subdirectories up to
the next subnet boundary) that noet recognizes as documents. Whitelist and blacklist
patterns let you narrow this set.

### 5.1 Filter semantics

| `whitelist` | `blacklist` | Result |
|---|---|---|
| empty | empty | accept all (default) |
| empty | non-empty | accept all **except** blacklist matches |
| non-empty | empty | accept **only** whitelist matches |
| non-empty | non-empty | accept whitelist matches **that are not** blacklist matches |

### 5.2 Pattern syntax

Patterns are globs anchored to the network directory:

| Pattern | Matches |
|---|---|
| `scratch.md` | exactly `scratch.md` in the network root |
| `scratch/**` | everything inside a `scratch/` subdirectory |
| `**/*.draft.md` | any `.draft.md` file at any depth |
| `generated/**` | everything inside `generated/` |
| `docs/**/*.md` | any `.md` file inside `docs/` at any depth |

**Anchoring**: patterns are relative to the network directory. `generated/**` matches
`<this_network>/generated/foo.yaml` but not `<other_network>/generated/foo.yaml`.

**Subnet directories**: A subnet directory (one containing its own `index.md`) is
matched using its `index.md` path. To blacklist the `drafts/` subnet, use either
`drafts/index.md` or `drafts/**` — both work.

### 5.3 Scoping

Filters are **per-network** and do not propagate to parent networks. A file excluded
by a parent network's blacklist will not appear in that parent's child list, but if
that file belongs to a subnet that the parent did not blacklist, the subnet's own
filters apply independently.

A blacklisted **subnet** is entirely excluded: its `index.md` is not parsed, its
children are not claimed, and no nodes from it appear in the BeliefBase.

> **Common pitfall — blacklists do not inherit.**  A blacklist on a root network's
> `index.md` only filters that network's direct children. Files inside accepted
> subnets are governed by the subnet's own `index.md`. If many subnets share
> the same exclusion pattern (e.g., `**/*.media/**` for pandoc media
> directories), the pattern must appear in **every** subnet's `index.md` —
> either by authoring it manually or by including it in the template that
> generates those files. Omitting it from subnets causes noet to walk and
> register every file as an asset, which can be very slow for large media
> trees (tens of thousands of images).

### 5.4 Examples

**Exclude generated output and scratch files:**
```toml
blacklist = ["generated/**", "scratch/**", "*.draft.md"]
```

**Include only authored documentation, exclude everything else:**
```toml
whitelist = ["docs/**/*.md", "specs/**/*.md"]
```

**Include a specific subtree but exclude auto-generated files within it:**
```toml
whitelist = ["src/**"]
blacklist = ["src/generated/**", "src/**/build/**"]
```

**Exclude a draft subnet while keeping all other subnets:**
```toml
blacklist = ["drafts/index.md"]
```

### 5.5 Diagnostics

When a file is excluded by a filter, noet emits a `ParseDiagnostic::info` message
naming the file. These appear in compile output at the `info` level and do not count
as warnings or errors. A clean build with filtered files still exits 0.

Malformed glob patterns (e.g. `[unclosed`) emit a `ParseDiagnostic::warning` and are
skipped; remaining valid patterns continue to apply.

---

## 6. Subnet Discovery and Nesting

Any subdirectory containing an `index.md` is automatically a subnet of its nearest
ancestor network. Subnet discovery is recursive: subnets can contain their own subnets.

```
repo/
├── index.md              ← root network
├── requirements/
│   ├── index.md          ← subnet (depth 1)
│   ├── functional/
│   │   ├── index.md      ← subnet (depth 2)
│   │   └── req-f-001.md
│   └── performance/
│       ├── index.md      ← subnet (depth 2)
│       └── req-p-001.md
└── architecture/
    ├── index.md          ← subnet (depth 1)
    └── overview.md
```

Plain subdirectories (no `index.md`) are not networks. Their files are owned by the
nearest ancestor network:

```
repo/
├── index.md              ← root network
└── assets/               ← plain subdirectory, NOT a network
    ├── diagram.png       ← asset (not a document node)
    └── notes.md          ← document owned by root network
```

Symlinked directories are followed. Symlink cycles are detected and skipped with a
warning.

---

## 7. Metadata Format Flexibility

noet auto-detects the frontmatter format. All three of the following are equivalent:

**TOML** (recommended — native format, best tooling support):
```markdown
---
id = "my-network"
title = "My Network"
blacklist = ["scratch/**"]
---
```

**YAML**:
```markdown
---
id: my-network
title: My Network
blacklist:
  - scratch/**
---
```

**JSON**:
```markdown
---
{
  "id": "my-network",
  "title": "My Network",
  "blacklist": ["scratch/**"]
}
---
```

Detection order: JSON → YAML → TOML. If your TOML uses `key = value` syntax (which
fails JSON and YAML parsing), it will be correctly parsed on the TOML fallback.

---

## 8. URL Aliasing

A cross-reference written as an external URL or a host-absolute path normally
resolves to nothing internal: noet mints an `External|Trace` stub to hold the
link target and the citation dead-ends there. **URL aliasing** lets a node claim
such a string as one of its own addresses, so a link written
`[TICKET-1101](https://tracker.example.com/browse/TICKET-1101)` resolves to the
internal node instead of a stub.

The user-visible benefit: a corpus imported from an external system keeps citing
that system's URLs — in prose, in generated tables, in text pasted from tickets —
and those citations still land on the internal node. No rewriting of link text is
required, and the graph gains real edges where it would otherwise hold orphan
stubs.

Two mechanisms produce aliases; a third controls only how they are displayed:

| Field | Where | Effect |
|---|---|---|
| `url_aliases` | document frontmatter | explicit list of URL/path strings for that node |
| `alias-template` | network `index.md` | derives one alias per descendant node from its frontmatter |
| `alias-base-url` | network `index.md` | display-only host prefix; does not affect resolution |
| `alias-scope` | network `index.md` | which descendants `alias-template` applies to |

`url_aliases` and `alias-template` compose additively: a node may carry both.

### 8.1 `url_aliases`

**Type**: array of strings  
**Where**: the frontmatter of any document — this is a per-node field, not a
network-level one, though a network's `index.md` may carry it like any other
document  
**Default**: absent — no aliases

Each entry is registered as an address for that document's root node. Entries may
be full URLs or host-absolute paths.

```toml
---
id = "ticket-1101"
title = "Widget alignment drifts under load"
url_aliases = [
  "https://tracker.example.com/browse/PROJ-1101",
  "/tickets/PROJ-1101",
]
---
```

The field round-trips: it is written back to the source frontmatter unchanged on
write-back. An empty array and an absent field are equivalent — both produce no
aliases.

Entries apply to the **document's root node**. A heading node cannot declare
`url_aliases` through a `[sections."..."]` table; use `alias-template` (§8.2) to
reach headings.

### 8.2 `alias-template`

**Type**: string containing `{{ field }}` placeholders  
**Where**: a network's `index.md` frontmatter  
**Applies to**: every descendant document beneath the declaring network, and
every node within each of those documents (subject to `alias-scope`, §8.4)

The template is evaluated once per node against that node's frontmatter. On
success the result becomes an alias for the node; on failure the node simply gets
no alias.

```toml
alias-template = "https://tracker.example.com/browse/{{ id | upper }}"
```

Inheritance walks *up* the directory tree: a document uses the `alias-template`
of the nearest ancestor network that declares one. Only ancestors are consulted,
so a subnet that declares its own template overrides the root's for everything
beneath it.

#### Substitution rules

| Rule | Example | Notes |
|---|---|---|
| Top-level frontmatter key | `{{ slug }}` | looks up the node's `slug` field |
| Dotted path into a sub-table | `{{ payload.slug }}` | navigates nested TOML tables |
| Multiple placeholders | `{{ base }}/browse/{{ id }}` | all must resolve |
| Whitespace inside braces | `{{id}}`, `{{ id }}` | both accepted |

A placeholder name may contain letters, digits, `_`, and `.`. Values are read
from the node's own frontmatter — for a heading node, that means the metadata
merged onto that heading, not the document root's.

**Value coercion**: strings are used as-is. Integers, floats, and booleans are
coerced to their string form. Arrays and tables cannot be coerced; a placeholder
resolving to one causes the whole template to fail for that node.

**All-or-nothing evaluation**: if *any* placeholder fails to resolve — the key is
missing, or its value is a non-coercible type — the template produces no alias
for that node. This is not an error: it is logged at debug level and the node is
skipped. A network whose template reads `{{ slug }}` therefore aliases exactly
those descendants that carry a `slug` field, and silently ignores the rest.

#### Filters

One filter is available:

| Filter | Syntax | Effect |
|---|---|---|
| `upper` | `{{ id \| upper }}` | uppercases the resolved value |

This is the complete set. An unrecognised filter name causes the placeholder not
to match the substitution pattern at all, so the literal `{{ ... }}` text is left
in the alias string.

#### Synthetic path variables

Two variables are injected before evaluation, derived from a node's *location*
rather than its frontmatter. They let a template address a page by where it lives,
which matters because most corpora have no hand-maintained slug field.

| Variable | Value for `guide/setup.md` | Value for the network `guide/` |
|---|---|---|
| `__path` | `guide/setup.md` | `guide` |
| `__html_path` | `guide/setup.html` | `guide` |

Both are relative to the directory of the network that declared the
`alias-template`, so one template on a root `index.md` gives every descendant its
own alias:

```toml
alias-template = "https://docs.example.com/{{ __html_path }}"
```

`__html_path` maps source extensions to their rendered `.html` form. For a
**network** node it deliberately yields the bare directory (`guide`) rather than
`guide/index.html`, because that is the spelling a static site serves and the
spelling documents actually cite.

A real frontmatter key of the same name wins: the synthetic values are only
injected when the key is absent. The variables are computed against a scratch
copy of the frontmatter and are never written back to the source file.

**The declaring network aliases itself** when — and only when — its template
mentions `__path` or `__html_path`. For that node both variables are empty, so a
template ending in `{{ __html_path }}` collapses to its bare prefix, which is the
network's own published URL. A purely frontmatter-driven template
(`{{ slug }}`, `{{ id | upper }}`) describes descendants, not the network, and is
not self-applied even if it happens to evaluate. If a path-driven template
evaluates to the empty string for the declaring network, a `warning` diagnostic is
emitted and self-registration is skipped.

### 8.3 `alias-base-url`

**Type**: string  
**Where**: a network's `index.md` frontmatter, alongside `alias-template`  
**Affects**: display only

Provides a host prefix for bare-path aliases when they are rendered in the
viewer's metadata panel. It has **no effect on resolution**: the alias is
registered, indexed, and matched exactly as the template produced it. A citation
must match the registered string, not the prefixed one.

```toml
alias-template = "/en-US/docs/{{ slug }}"
alias-base-url = "https://docs.example.com"
```

Here the alias `/en-US/docs/widgets` is what resolves; the base URL only makes
the metadata-panel entry clickable. It is unnecessary when the template already
produces full URLs.

> [!NOTE]
> The field is parsed and stored in the alias config, but no consumer in this
> repository currently reads it — the viewer's "External Link(s)" panel renders
> the registered alias strings directly. Authors whose templates emit bare paths
> should expect those paths to display unprefixed until a consumer wires it up.

### 8.4 `alias-scope`

**Type**: string — `"submap"` or `"explicit"`  
**Where**: a network's `index.md` frontmatter, alongside `alias-template`  
**Default**: `"submap"`

Controls which descendant nodes an `alias-template` reaches. It is read only when
`alias-template` is also present; on its own it has no effect.
Controls which descendant nodes an `alias-template` reaches. It is read only when
`alias-template` is also present; on its own it has no effect.

| Value | Meaning |
|---|---|
| `submap` | every node in every descendant document — document roots *and* every heading |
| `explicit` | only nodes that opt in with `alias = true` |

Value matching is case-insensitive and surrounding whitespace is trimmed, so
`explicit`, `Explicit`, and `" Explicit "` are all accepted. An **unrecognised
value** emits a `warning` diagnostic naming the offending string and the network
directory, then falls back to `submap`.

`submap` is right for a network whose headings carry meaningful external keys —
hazard reports whose `h2` anchors are ticket identifiers, for instance. It is
wrong for a network whose descendants contain machine-generated headings: a
corpus of imported slide decks, where every slide becomes an `h2` with a
positional anchor, will register an alias for each one. Those anchors are not
document-unique either, so many documents collide on the same alias. Use
`explicit` there.

#### Per-node override

Any node may override the network default with a boolean `alias` field. The
node's own value always wins, in both directions: `alias = true` opts a node in
under `explicit`, and `alias = false` opts it out under `submap`.

For a **document's root node**, write it in the document frontmatter:

```toml
---
id = "doc-root"
alias = true
---
```

For a **heading node**, write it in a `[sections."..."]` table in the document
root's frontmatter. The table key is a NodeKey — `id://anchor` or a bare anchor —
not the `#anchor` form used in the heading itself:

```markdown
---
id = "doc-root"

[sections."id://sec-two"]
alias = true
---

## First {#sec-one}

## Second {#sec-two}
```

Under `explicit`, only `sec-two` is aliased here.

The declaring network's self-alias honours `alias-scope` too: under `explicit`
the network must carry `alias = true` in its own `index.md` frontmatter to alias
itself.

### 8.5 Case sensitivity

URL paths are case-sensitive (RFC 3986 §6.2.2.1) and noet treats them that way.
An alias must match the citing URL **exactly**. `.../browse/PROJ-1101` and
`.../browse/proj-1101` are two different addresses; registering one does not
resolve citations of the other.

This is why the `| upper` filter exists: anchor ids are slugified to lowercase,
so a template deriving an uppercase external key from an anchor needs
`{{ id | upper }}` to reproduce the cited spelling.

One normalisation *is* applied, to both the registered alias and the citation, so
the two meet on one key: a trailing slash is dropped, and `.`/`..` segments are
resolved. An alias written `https://ex.com/x/doc/` and a citation of
`https://ex.com/x/doc` therefore match.

### 8.6 Collision behaviour

When a node registers an alias that another node already holds, the outcome
depends on what the incumbent is:

| Incumbent | Outcome |
|---|---|
| The same node (re-parse) | idempotent — the edge is re-emitted, no diagnostic |
| An `External\|Trace` stub | the content node wins; the stub is absorbed and removed |
| Another content node | **first one wins**; the second is skipped with a `warning` |

The content-node-beats-stub rule is the ordinary case and is silent: it is
exactly the behaviour URL aliasing exists to produce. The stub is retired rather
than left to coexist, so the alias resolves to one node.

A content-node collision emits a diagnostic of the form:

```
URL alias collision: '<alias>' is already registered to node <bid>;
this node (<bid>) will not be reachable via this alias.
```

The losing node keeps all its other addresses — only the colliding alias is
skipped. Resolve it by making the aliases distinct.

> Collisions between documents parsed in different batches converge across parse
> epochs rather than being detected on first sight, so which node "wins" is
> determined by parse order.

### 8.7 Worked example

A network of documents derived from an issue tracker. Each document carries the
tracker's issue key in its `id`; the network derives the tracker URL from it.

**`tickets/index.md`:**

```markdown
---
id = "widget-project-tickets"
title = "Widget Project Tickets"
text = "Issues imported from the project tracker."

tracker_base_url = "https://tracker.example.com"
alias-template = "{{ tracker_base_url }}/browse/{{ id | upper }}"
alias-scope = "explicit"
---

# Widget Project Tickets

Each document below mirrors one tracker issue and claims that issue's URL as an
alias, so prose citing the tracker link resolves here.

````{network_children}
````
```

Note that `tracker_base_url` is an ordinary frontmatter field, not a reserved
name. It resolves per node, so a document may override the host by declaring its
own `tracker_base_url`.

> [!IMPORTANT]
> `alias-template` is evaluated against each **descendant's** frontmatter, not the
> network's. A field declared only on `index.md` — like `tracker_base_url` above —
> will not resolve for descendants unless they carry it too. Put shared constants
> in the template string itself unless every descendant is known to define them.

**`tickets/proj-123.md`:**

```markdown
---
id = "proj-123"
title = "Widget alignment drifts under load"
tracker_base_url = "https://tracker.example.com"
alias = true
---

# Widget alignment drifts under load

Under sustained load the widget assembly drifts out of alignment.
```

The template evaluates to `https://tracker.example.com/browse/PROJ-123` — note
`| upper` recovering the tracker's uppercase key from the lowercase `id`. Because
the network's scope is `explicit`, only the document root (which carries
`alias = true`) is aliased; the `#` headings inside are not.

**`tickets/proj-124.md`** adds an explicit alias alongside the derived one:

```markdown
---
id = "proj-124"
title = "Fastener torque spec is ambiguous"
tracker_base_url = "https://tracker.example.com"
alias = true
url_aliases = ["https://tracker.example.com/browse/PROJ-99"]
---
```

This node now answers to two addresses: the derived
`https://tracker.example.com/browse/PROJ-124` and the explicit `.../PROJ-99` — a
superseded key that older documents still cite.

Any document in the corpus may now write:

```markdown
See [PROJ-123](https://tracker.example.com/browse/PROJ-123) for the drift report.
```

and the link resolves to `tickets/proj-123.md` instead of minting a stub.

---

## 9. Minimal and Full Examples

### Minimal network

```markdown
---
id = "my-network"
---
```

This is the smallest valid `index.md`. `title` defaults to a slug-derived display
name; body is empty; all children are included.

### Complete example

```markdown
---
id = "system-requirements"
title = "System Requirements"
text = "Functional and performance requirements for the Widget Project."
blacklist = ["generated/**", "archive/**"]
---

# System Requirements

This network contains all system-level requirements organized by subsystem.

## Overview

Requirements are authored in individual `.md` files and linked via the
`[[req-id]]` cross-reference format. Traceability to verification events
is maintained automatically.

## Contents

````{network_children}
````
```

### Subnet with whitelist

```markdown
---
id = "active-requirements"
title = "Active Requirements"
whitelist = ["req-*.md", "subsystems/**"]
blacklist = ["subsystems/deprecated/**"]
---

# Active Requirements

Only currently active requirement documents. Draft and archived requirements
are excluded from this network.

````{network_children}
````
```

---

## 10. Initialization

To create a new network from the command line:

```sh
noet init <directory>
```

This creates `<directory>/index.md` with a generated `id`, prompts for a title, and
leaves the body empty. The `--id` and `--title` flags skip the prompts:

```sh
noet init requirements --id system-requirements --title "System Requirements"
```

---

## 11. Common Mistakes

**Missing `id`**: The network will fail to compile with an error. Every `index.md`
must have `id = "..."` in its frontmatter.

**Duplicate `id` within a corpus**: IDs must be unique. noet detects collisions and
assigns a disambiguated ID, appending a numeric suffix. The collision is surfaced as a
warning. Resolve by choosing distinct IDs.

**Modifying `bid` by hand**: The `bid` field is system-managed. Changing it breaks
cross-references from other documents and will cause BID conflicts on next compile.
Leave it alone after it is written.

**Blacklisting a file that is already excluded by parent**: Redundant blacklist entries
are harmless but add noise. A file can only be included if its entire ancestor chain
of networks accepts it.

**Using absolute paths in patterns**: Patterns are always network-relative. `/generated/**`
will not match anything — use `generated/**` instead.

**Expecting `{network_children}` to update in real time**: The child listing is
generated during the deferred HTML pass at the end of compilation. It reflects the
state of the corpus at compile time, not live filesystem state.

**Expecting an alias to match case-insensitively**: URL paths are case-sensitive.
`.../browse/proj-123` will not resolve a citation of `.../browse/PROJ-123`. Anchor
ids are slugified to lowercase, so a template deriving an uppercase external key
from one needs `{{ id | upper }}`. See §8.5.

**Referencing a network-only field from `alias-template`**: The template is
evaluated against each *descendant's* frontmatter, not the declaring network's. A
constant declared only on `index.md` resolves for no descendant, and the whole
template silently produces no aliases. Put shared constants in the template string
itself, or repeat them on each descendant. See §8.7.

**Leaving `alias-scope` at its default over machine-generated headings**: Under
the `submap` default, `alias-template` is applied to every heading in every
descendant document. A corpus with positional auto-generated anchors will register
one alias per heading, and those anchors are rarely document-unique, so many
collide. Set `alias-scope = "explicit"` and opt nodes in individually. See §8.4.

**Using the `#anchor` form as a `[sections]` key**: The table key is a NodeKey —
`[sections."id://sec-two"]` or a bare anchor — not `[sections."#sec-two"]`. A
mismatched key means the heading's `alias` override is never found. See §8.4.

**Expecting `alias-base-url` to affect resolution**: It is display-only. The alias
is matched exactly as `alias-template` produced it, without the prefix. If
citations use full URLs, the template must produce full URLs. See §8.3.

---

## 12. Multi-Version Deployments

### 12.1 Overview

noet supports serving multiple documentation versions side-by-side. Each version
is a complete, self-contained site build rendered at a specific git state. A
version-selector dropdown in the SPA viewer lets readers switch between versions.

The design separates concerns:

- **noet** renders one version at a time and provides the viewer-side version selector
- **CI** orchestrates multi-version builds, directory layout, and manifest assembly

### 12.2 Directory layout

```
output/
  index.html              ← redirect to default version
  versions.json           ← manifest consumed by the version selector
  v/
    latest/               ← HEAD build (or default branch)
      index.html
      beliefbase/
      pages/
    v2.0.0/
      index.html
      beliefbase/
      pages/
    v1.0.0/
      ...
```

The `v/` prefix is part of the contract — the viewer's version-selector JS
identifies versioned deployments by matching `/v/<version>/` in the URL pathname.

Each version directory is a self-contained noet site build produced by:

```sh
noet parse --base-url <base>/v/<tag>/ --html-output output/v/<tag>/
```

### 12.3 `versions.json` schema

```json
{
  "versions": [
    {
      "label": "Latest (main)",
      "path": "v/latest/"
    },
    {
      "label": "v2.0.0",
      "path": "v/v2.0.0/"
    }
  ]
}
```

Fields:

- `label` (string, required) — display text in the version dropdown
- `path` (string, required) — relative path from the site root to this version's
  directory (must include trailing `/`)

The schema is intentionally minimal. Consuming projects may add additional fields
(e.g., `commit`, `date`) — the viewer ignores unknown keys. The dropdown order
matches the array order.

### 12.4 `assemble-versions.sh`

noet provides `scripts/assemble-versions.sh` to generate `versions.json` and a
root `index.html` redirect from a directory of built versions.

```sh
scripts/assemble-versions.sh <output-dir> <label>:<dirname> [<label>:<dirname> ...]
```

Example:

```sh
scripts/assemble-versions.sh _site \
  "Latest (main):latest" \
  "v2.0.0:v2.0.0" \
  "v1.0.0:v1.0.0"
```

The script:

- Checks that each `<output-dir>/v/<dirname>/index.html` exists
- Skips entries with missing content (with a warning)
- Writes `versions.json` at the output root
- Writes `index.html` with a meta-refresh redirect to the first listed version
- Requires `jq`

### 12.5 Version selector behavior

The version-selector dropdown (`assets/viewer/version-selector.js`) auto-detects
versioned deployments:

- If the current URL contains `/v/<version>/`, it fetches `versions.json` from the
  site root
- If found with ≥2 entries, it renders a `<select>` dropdown in the navigation header
- On selection change, it navigates to the equivalent page in the selected version,
  preserving the hash fragment
- If the URL has no `/v/` segment, or the manifest fetch fails, the selector stays
  hidden — single-version deployments work unchanged

### 12.6 Example CI pattern

A typical multi-version CI workflow:

1. Maintain a list of version refs (tags, branches, commit SHAs) in the repo
2. For each version, check out the source at that ref and run `noet parse` with a
   version-specific `--base-url` and `--html-output`
3. Run `assemble-versions.sh` to generate the manifest
4. Deploy the combined output

```sh
# Build two versions
noet parse --base-url https://example.com/v/latest/ \
  --html-output output/v/latest/ src/

git checkout v2.0.0
noet parse --base-url https://example.com/v/v2.0.0/ \
  --html-output output/v/v2.0.0/ src/

# Assemble manifest
assemble-versions.sh output "Latest (main):latest" "v2.0.0:v2.0.0"
```

---

## 13. References

- `src/codec/network.rs` — `NetworkCodec` implementation; `detect_network_file`;
  `AliasTemplateConfig`, `AliasScope`, `evaluate_alias_template`
- `src/codec/proto_index.rs` — `net_dir_partition`, `ProtoIndex::build`,
  `ProtoIndex::ancestor_meta_as`
- `src/codec/belief_ir.rs` — `url_aliases` frontmatter extraction
- `src/codec/builder.rs` — `GraphBuilder::push` alias registration and collision handling
- `docs/design/core/beliefbase_architecture.md` §3.2 — codec dispatch and CLAIM_MAP
- `docs/design/codecs/myst_directive_architecture.md` — `{network_children}` and other directives
- Issue 72: Network Child Filtering — whitelist/blacklist implementation details
