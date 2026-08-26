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

## 8. Minimal and Full Examples

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
text = "Functional and performance requirements for the Haven spacecraft."
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

## 9. Initialization

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

## 10. Common Mistakes

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

---

## 11. Multi-Version Deployments

### 11.1 Overview

noet supports serving multiple documentation versions side-by-side. Each version
is a complete, self-contained site build rendered at a specific git state. A
version-selector dropdown in the SPA viewer lets readers switch between versions.

The design separates concerns:

- **noet** renders one version at a time and provides the viewer-side version selector
- **CI** orchestrates multi-version builds, directory layout, and manifest assembly

### 11.2 Directory layout

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

### 11.3 `versions.json` schema

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

### 11.4 `assemble-versions.sh`

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

### 11.5 Version selector behavior

The version-selector dropdown (`assets/viewer/version-selector.js`) auto-detects
versioned deployments:

- If the current URL contains `/v/<version>/`, it fetches `versions.json` from the
  site root
- If found with ≥2 entries, it renders a `<select>` dropdown in the navigation header
- On selection change, it navigates to the equivalent page in the selected version,
  preserving the hash fragment
- If the URL has no `/v/` segment, or the manifest fetch fails, the selector stays
  hidden — single-version deployments work unchanged

### 11.6 Example CI pattern

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

## 12. References

- `src/codec/network.rs` — `NetworkCodec` implementation; `detect_network_file`
- `src/codec/proto_index.rs` — `net_dir_partition`, `ProtoIndex::build`
- `docs/design/beliefbase_architecture.md` §3.2 — codec dispatch and CLAIM_MAP
- `docs/design/myst_directive_architecture.md` — `{network_children}` and other directives
- Issue 72: Network Child Filtering — whitelist/blacklist implementation details
