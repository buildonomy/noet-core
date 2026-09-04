# XLSX / ODS Codec: Index Tab Schema Reference

**Version**: 0.2  
**Status**: Active  
**Applies to**: `XlsxCodec` (`src/codec/xlsx/`) — requires `--features xlsx`

---

## 1. Purpose

This document is the authoritative author reference for ingesting `.xlsx` and `.ods`
spreadsheet files as first-class corpus nodes via the noet `XlsxCodec`. It covers:

- How the codec discovers and opens workbooks
- The `index` tab convention and schema grammar
- Column roles, lazy defaults, and `ColumnKind` resolution
- Text-as-Markdown: link extraction and graph edges from cell content
- The `text_template` field for multi-column composition
- Opaque tabs, row overflow, and CSV asset export
- BID stability and write-back behaviour (including relation-bref columns)
- Viewer behaviour: tab switching, hash routing, relation links
- Validation, diagnostics, and known limitations

For implementation details see `src/codec/xlsx/codec.rs` and
`src/codec/xlsx/schema.rs`.

---

## 2. How the Codec Finds Your Workbook

`noet compile` discovers `.xlsx` and `.ods` files through the same directory walk
that finds `.md` files. Once found, the codec runs two phases:

1. **`proto()`** — opens the workbook, reads cell A1 of the `index` tab, and
   deserialises it as the schema declaration. If no `index` tab exists the file is
   registered as a binary asset: no document nodes are emitted, no error, no warning.

2. **`parse()`** — re-opens the workbook, emits the full node hierarchy (workbook →
   tab → row), generates CSV exports of opaque tabs, and collects state for write-back.

Nothing in the source directory is modified unless `noet compile --write` is passed.
BID annotations and resolved relation brefs are written back via hidden reserved
columns (see §7).

---

## 3. The `index` Tab

### 3.1 What it is

The `index` tab is a **reserved worksheet** whose sole content is a schema declaration
in cell A1. It is never parsed as data rows — it is metadata about the workbook.

Rules:

- The tab must be named exactly `index` (lowercase, no surrounding spaces).
- Cell A1 must contain the schema declaration (see §4).
- All other cells in the `index` tab are ignored.
- The `index` tab may appear anywhere in the tab order.

### 3.2 Creating the `index` tab

In Excel: right-click any tab → Insert → Worksheet, name it `index`.  
In LibreOffice Calc: right-click any tab → Insert Sheet, name it `index`.

Type or paste the schema YAML into cell A1. Press Alt+Enter (Excel) or Ctrl+Enter
(LibreOffice) to insert line breaks within the cell so you can verify the full content.
The formula bar always shows the complete value regardless of cell display truncation.

---

## 4. Schema Grammar

Cell A1 accepts YAML (preferred), TOML, or JSON. The codec tries YAML first, then
JSON, then TOML. YAML is the canonical format; all examples in this document use it.

### 4.1 Minimal example

```yaml
title: "Widget Project Requirements"
tabs:
  - name: "Functional Requirements"
```

This is fully valid. The first column of `Functional Requirements` becomes the node
title automatically; all other columns are stored as payload. No `schema` key is
required.

### 4.2 Explicit schema example

```yaml
title: "Widget Project Requirements"
tabs:
  - name: "Functional Requirements"
    schema:
      - col: "Description"
        role: text
      - col: "Implements"
        role: relation
        weight: pragmatic
      - col: "Category"
        role: payload
```

Only columns that differ from their lazy defaults need to be declared. The `Title`
column is not listed — it is inferred as `role: title` because it is the first
non-reserved column in the header row (see §5).

### 4.3 Full field reference

```yaml
# ── Root fields ────────────────────────────────────────────────────────────

# REQUIRED. Sets the title of the workbook node in the belief graph.
title: "Widget Project Requirements"

# OPTIONAL. Stable semantic identifier for the workbook node.
# Processed through to_anchor() before storage (NFKC-normalised, lowercased,
# whitespace → hyphen, punctuation stripped). Consistent with BeliefNode::id.
# When provided, GraphBuilder uses it as a collision-resistant lookup key so
# the workbook node retains the same BID even if the file is renamed or moved.
# bid and bref are never declared — bid is injected by --write into the schema
# YAML itself; bref is always derived from bid at runtime.
id: "widget-req"

# REQUIRED (but can be empty list). Ordered list of tab declarations.
# Tabs present in the workbook but absent from this list are "opaque" — they
# receive a single container node and their content is exported to
# .noet/derived/<workbook>__<tab>.csv (see §6).
tabs:

  # ── Tab declaration ───────────────────────────────────────────────────────

  - name: "Functional Requirements"   # REQUIRED. Case-sensitive; no leading/trailing spaces.

    # OPTIONAL. Compose multiple columns into a single Markdown text body using
    # {{ColumnName}} placeholders. When present, supersedes role: text columns
    # for body text composition. The composed string is parsed as Markdown so
    # links become graph edges (see §5.6). Placeholder matching is
    # case-insensitive after to_anchor() normalization; spaces inside braces are
    # trimmed so {{ Statement }} and {{statement}} both work.
    text_template: "{{Description}}\n\n**Rationale**: {{Rationale}}"

    # OPTIONAL. Explicit column role declarations. Columns not listed here
    # receive lazy defaults (see §5). An empty or absent schema is valid.
    schema:

      # Each entry declares one column. Fields:
      #   col:       REQUIRED. Header string as it appears in row 1 of the tab.
      #              Case-sensitive. Must match exactly (no leading/trailing spaces).
      #   role:      REQUIRED. One of: title, text, relation, payload.
      #   weight:    OPTIONAL. For role: relation only. One of: pragmatic, epistemic.
      #              Default: pragmatic.
      #   direction: OPTIONAL. For role: relation only. One of: upstream, downstream.
      #              Default: upstream.
      #   key:       OPTIONAL. For role: relation only. One of: auto, id, path, bid, bref.
      #              Default: auto.
      #   wrap:      OPTIONAL. Boolean. When true, the cell wraps in the HTML viewer.
      #              Default: false.

      - col: "Implements"
        role: relation
        weight: pragmatic       # assertion-backed (this is also the default)

      - col: "See Also"
        role: relation
        weight: epistemic       # evidence-backed cross-reference

      - col: "Priority"
        role: payload           # stored in node.payload["Priority"], not indexed

      # "Title"       → title (conventional name match; not listed)
      # "Description" → consumed by text_template; not listed in schema
      # "Rationale"   → consumed by text_template; not listed in schema

  - name: "Components"
    schema:
      - col: "Description"
        role: text

  # "Changelog" is absent → opaque tab (see §6)

  # ── Wildcard tab (applies to all unmatched tabs) ─────────────────────────

  # A tab declaration with name: "*" acts as a default schema for every tab
  # that has no exact-name match. The wildcard entry is never itself treated
  # as a real tab name. When a wildcard is present, unmatched tabs receive
  # the wildcard's schema and text_template instead of being treated as opaque.
  #
  # Example: apply the same schema to every tab without listing each one:
  # - name: "*"
  #   text_template: "{{Description}}\n\n**Rationale**: {{Rationale}}"
  #   schema:
  #     - col: "Implements"
  #       role: relation
```

---

## 5. Column Roles and Lazy Defaults

### 5.1 `ColumnKind` — the internal resolved role

The codec converts each column's declared `ColumnRole` (from the YAML schema) into a
`ColumnKind` (a codec-internal enum) during header-row processing. `ColumnKind` is
richer than `ColumnRole`: it carries resolved parameters inline and includes
system-managed variants that have no `ColumnRole` equivalent.

| `ColumnKind` variant | Corresponds to | Notes |
|---|---|---|
| `Title` | `role: title` | At most one per tab; becomes `node.title` |
| `Markdown` | `role: text` | Parsed as Markdown; links → graph edges |
| `Relation { weight, direction, key_format }` | `role: relation` | Emits graph edges |
| `RelationBref { relation_ir_key }` | system-managed | Hidden; written by `--write`; read for stable resolution |
| `Payload` | `role: payload`, and all reserved columns | Stored in `doc[ir_key]` |

Authors declare `ColumnRole` in YAML. `ColumnKind` is resolved at compile time and
is not user-facing. The `RelationBref` variant is invisible to authors — it is
created and managed entirely by the codec (see §7.3).

### 5.2 Lazy defaults — column role priority

Columns not listed in `schema` receive roles in this priority order:

1. **`__noet_*__` reserved columns** — detected automatically by header prefix;
   mapped to system fields (`bid`, `id`, `title`, `schema`, `kind`). See §5.5.
2. **Explicit schema declarations** — columns listed in `schema:` use their declared
   role. Reserved columns always win over explicit declarations for the same index.
3. **Layer-2 conventional name match** — unlisted columns whose header is `"title"`
   or `"text"` (case-insensitive) are automatically promoted to `role: title` or
   `role: text` respectively without appearing in the schema.
4. **Positional title fallback** — when no conventional `"title"` header exists and
   no explicit `role: title` has been declared, the first non-reserved, non-conventional
   unlisted column gets `role: title`.
5. **All remaining unlisted columns** → `role: payload`.

A tab with no `schema` key at all is valid. A `ParseDiagnostic::Warning` is emitted
when no `title`-role column can be determined (all columns are reserved, or the sheet
has no header row).

The conventional name table currently recognises two names:

| Header (case-insensitive) | Auto role |
|---|---|
| `title` | `role: title` |
| `text` | `role: text` |

Columns named `description`, `category`, or any other word are **not** auto-promoted —
they fall through to `role: payload` unless explicitly declared.

### 5.3 Wildcard tab (`name: "*"`)

A tab declaration with `name: "*"` acts as a default schema applied to every workbook
tab that has no exact-name match in the `tabs` list:

```yaml
title: "Widget Project"
tabs:
  - name: "Special"          # exact match: uses this schema
    schema:
      - col: "Notes"
        role: text
  - name: "*"                # default: applies to every other tab
    text_template: "{{Description}}\n\n**Rationale**: {{Rationale}}"
    schema:
      - col: "Implements"
        role: relation
```

Lookup order: exact `name` match → wildcard → opaque tab.

When a wildcard is present, no tab is treated as opaque — the wildcard's schema and
`text_template` are applied to every unmatched tab. The actual sheet name (not `"*"`)
is used for the tab container node title, provenance payload, and the derived CSV
filename.

### 5.4 `title` — Node title

```yaml
- col: "Title"
  role: title
```

- **At most one** `title` column per tab schema. When absent from the explicit schema,
  the first non-reserved column matching the conventional name `"title"` (case-insensitive)
  is promoted automatically; if no such column exists, the first non-reserved unlisted
  column takes the role positionally.
- The cell value becomes the row node's `title` field — the primary display name in
  search results and the HTML viewer.
- If the `title` cell is empty, the codec auto-generates a title of the form
  `<TabName>:<row_number>` and emits a `Warning` diagnostic. The row node is still
  emitted; nothing is silently dropped.
- `__noet_title__` overrides the title-role column when present (see §5.5).

### 5.5 `text` — Body text (search-indexed, Markdown-parsed)

```yaml
- col: "Description"
  role: text
- col: "Rationale"
  role: text
```

- Multiple `text` columns are allowed. Their values are joined with `\n\n` and stored
  in `node.payload["text"]`.
- Cell content is **parsed as Markdown**. Links in the text become upstream graph edges
  (`IntermediateRelation` entries in `node.upstream`), resolved at `inject_context`
  time — identical to how links in Markdown documents work.
- Text content is included in the full-text search index.
- **Superseded by `text_template`** when that field is present on the tab (see §5.9).
- A column named `"text"` (case-insensitive) receives this role automatically without
  appearing in the schema (layer-2 conventional name match, §5.2).

### 5.6 Reserved columns (`__noet_<property>__`)

Columns whose header exactly matches `__noet_<property>__` are detected automatically
in the header row, regardless of whether they appear in the `schema` list. They map
directly to `BeliefNode` fields:

| Column header      | BeliefNode field | Behaviour                                               |
|--------------------|------------------|---------------------------------------------------------|
| `__noet_bid__`     | `bid`            | Injected by `--write`; read back on next parse for BID stability |
| `__noet_id__`      | `id`             | User-authored stable ID; processed by `to_anchor()`     |
| `__noet_title__`   | `title`          | Overrides the title-role column when non-empty          |
| `__noet_schema__`  | `schema`         | Schema string for schema-aware nodes                    |
| `__noet_kind__`    | `kind`           | `BeliefKind` set; parsed as comma-separated kind names  |

Reserved columns are:
- Never included in `payload`.
- Never emitted as `text` content.
- Silently skipped when their cell is empty.
- **Hidden** in the spreadsheet after write-back (see §7) so they do not clutter the
  normal data view. They are fully readable by calamine on the next compile.

An unrecognised `__noet_<x>__` pattern emits a `ParseDiagnostic::Warning` and falls
back to `payload`.

### 5.7 `relation` — Graph edge

```yaml
- col: "Implements"
  role: relation
  weight: pragmatic       # or: epistemic
  direction: upstream     # or: downstream  (default: upstream)
  key: auto               # or: id, path, bid, bref
```

- Cell value is parsed as a noet node reference. Multiple references may be separated
  by semicolons: `"abc123de; def456fg"`.
- Each resolved reference becomes a graph edge in the specified `weight` kind and
  `direction`.

**`weight`** — edge assertion strength:
  - `pragmatic` — assertion-backed (default). Use for traceability, implementation
    claims, verification links.
  - `epistemic` — evidence-backed. Use for design cross-references, rationale links,
    document citations.

**`direction`** — which end of the edge this row node occupies:
  - `upstream` (default) — the cell value identifies a node this row **derives from or
    is constrained by** (the more abstract/parent end). Stored in `IRNode::upstream`.
    Use when the row *consumes* the reference: "this requirement implements that
    top-level requirement", "this component satisfies that interface".
  - `downstream` — the cell value identifies a node that **derives from or is
    constrained by** this row (the more concrete/child end). Stored in
    `IRNode::downstream`. Use when the row *produces* the reference: "this requirement
    is verified by these test cases", "this interface is implemented by these components".

**`key`** — explicit `NodeKey` type hint:

| `key:` value | NodeKey variant  | Use when cell contains                    |
|--------------|------------------|-------------------------------------------|
| `auto`       | heuristic        | BID → Bref → Id (default; works for brefs and BIDs) |
| `id`         | `NodeKey::Id`    | Semantic slug (`code-generation`)         |
| `path`       | `NodeKey::Path`  | Repo-relative file path                   |
| `bid`        | `NodeKey::Bid`   | Full UUID string                          |
| `bref`       | `NodeKey::Bref`  | 12-char hex bref                          |

When `key: auto` (the default), `NodeKey::from_str` uses its bare-string heuristic:
BID → Bref → Id. This works correctly for brefs and BIDs but produces `NodeKey::Id`
for plain-text labels like `"Code Generation"` — which may be the desired behaviour
(resolve to a corpus node by semantic id) or may not. Use an explicit `key:` value
when the cell format is known.

- Unresolvable references emit a `Warning` and are omitted from the edge list. The row
  node is still emitted.
- Cell formulas are resolved to their computed value before parsing.

### 5.8 `payload` — Arbitrary storage (not indexed)

```yaml
- col: "Priority"
  role: payload
- col: "External ID"
  role: payload
```

- Stored in `node.payload["<col_name>"]` with the column header (normalized via
  `to_anchor()`) as the key.
- Not included in the search index.
- Use for structured metadata that tooling needs but that noet does not reason about
  (e.g. ticket numbers, revision letters, external tool IDs).

### 5.9 `text_template` — Multi-column Markdown body

```yaml
- name: "Functional Requirements"
  text_template: "{{Description}}\n\n**Rationale**: {{Rationale}}\n\n**Criteria**: {{Acceptance}}"
  schema:
    - col: "Implements"
      role: relation
```

The `text_template` field on a tab declaration composes multiple columns into a single
Markdown body string using `{{ColumnName}}` placeholders. When present:

- It **supersedes** individual `role: text` column declarations for body text composition.
- Placeholder matching is **case-insensitive** after `to_anchor()` normalisation: the
  key `{{Statement}}`, `{{ statement }}`, and `{{STATEMENT}}` all resolve to the same
  column. Spaces inside braces are trimmed before lookup.
- Column names absent from the header row are replaced with an empty string silently.
- The composed string is **parsed as Markdown** — links become upstream graph edges.
- The composed string is stored in `node.payload["text"]` and is search-indexed.
- **Empty-render detection**: the template is applied with all placeholder values
  replaced by empty strings to produce a reference "empty render". If a row's composed
  output (after trimming) equals the empty render, the row is considered to have no
  meaningful text content and is **skipped** entirely — not emitted as a node. A
  `Warning` diagnostic is emitted when the skipped row has a non-empty title, relation
  cells, or non-empty payload cells, since that indicates data was present but discarded.
  Rows that are completely blank (all cells empty) are silently skipped with no diagnostic.

Cell values are substituted verbatim into the template. If a cell value itself contains
`{{`, it is inserted as-is — only the template string is scanned for placeholders.

---

## 6. Opaque Tabs and Row Overflow

### 6.1 Opaque tabs and the wildcard override

A tab is treated as opaque only when **no matching schema exists** — neither an exact
`name` match nor a `name: "*"` wildcard entry (see §5.3). When a wildcard is present,
no tab is treated as opaque.

### 6.2 Opaque tabs (no wildcard)

A tab present in the workbook but **absent from the `tabs` list** is called an
**opaque tab**. Opaque tabs receive:

1. **A single container node** (`BeliefKind::Symbol`, heading=3) with the tab name
   as its title. No row nodes are emitted.

2. **A CSV export** written to `.noet/derived/<workbook_stem>__<tab_name>.csv`
   where non-alphanumeric, non-hyphen characters in the tab name are replaced with `_`.
   The container node carries an `Epistemic` upstream relation pointing to this CSV
   asset, making the full dataset traversable from the graph.

3. **No errors or warnings** — this is the intentional "I know this tab exists but I
   do not want noet to parse its rows" pattern.

Example: a workbook with tabs `index`, `Functional Requirements`, `Components`,
`Changelog`, where the schema declares only `Functional Requirements` and `Components`:

```
workbook node                     (Document, heading=2)
  ├── Functional Requirements     (Symbol, heading=3, row nodes…)
  ├── Components                  (Symbol, heading=3, row nodes…)
  └── Changelog                   (Symbol, heading=3, opaque)
        upstream → .noet/derived/requirements__Changelog.csv [Epistemic]
```

### 6.3 `ignore: true` — Explicit opaque tab

Setting `ignore: true` on a tab entry forces opaque treatment even when a `schema:`
list is declared. The tab is excluded from row-node parsing and receives the same
output as an absent tab (§6.2):

1. **A single container node** (`BeliefKind::Symbol`, heading=3) with the tab name
   as its title. No row nodes are emitted.

2. **A CSV export** written to `.noet/derived/<workbook_stem>__<tab_name>.csv`.
   The container node carries an `Epistemic` upstream relation pointing to this asset.

3. **No errors or warnings** — `ignore: true` is an intentional author declaration.

**Primary use cases**: template rows, admin sheets, lookup tables that should not
become corpus nodes but whose presence should be documented and shielded from the
wildcard (`name: "*"`).

```yaml
tabs:
  - name: "Template"
    ignore: true
  - name: "*"
    schema:
      - col: "Implements"
        role: relation
```

In the example above, `"Template"` is explicitly excluded from graph ingestion while
any other tab (except `index`) receives the wildcard schema. Without `ignore: true`,
the wildcard would pick up `"Template"` and emit row nodes from it.

A `schema:` list may still be declared alongside `ignore: true` — it is silently
ignored. This allows authors to document what the columns *would* mean without
activating parsing.

Default: `false`.

### 6.4 Row overflow

The architectural row limit per tab is `u16::MAX` (65 535), imposed by `PathMap`'s
`u16` sort-key space. A tab exceeding this limit is treated as an opaque tab:

1. **No row nodes are emitted** — only the tab container node.
2. The full tab (header + all rows) is exported to `.noet/derived/<workbook_stem>__<tab_name>.csv`.
3. A `ParseDiagnostic::Warning` is emitted naming the row count and the CSV path.
4. An `Epistemic` upstream relation is added to the tab container node pointing to
   the CSV asset — identical to the opaque-tab pattern.

The `.noet/` directory should be listed in `.gitignore` if you do not want to commit
derived outputs. noet emits an informational message if `.noet/` is not in `.gitignore`
but never modifies `.gitignore` itself — that decision belongs to the author.

---

## 7. BID Stability and Write-Back

After the first `noet compile --write`, the codec injects hidden columns into each
schema-declared tab to stabilize row identity across subsequent compiles. Authors
should never author or delete these columns manually.

### 7.1 `__noet_bid__` — Row BID injection

On the first compile, row nodes have no pre-existing BID. The codec sets a positional
`id` in `doc["id"]` at parse time (see §7.4), and `GraphBuilder::push()` generates a
fresh BID if no cache entry exists.

After running `noet compile --write`, the `__noet_bid__` column is written to each
schema-declared tab and hidden. On all subsequent compiles, `speculative_path_key`
finds the BID immediately — row insertions, reorderings, and renames do not affect it.

**Do not delete or rename `__noet_bid__`.**  If a cell is empty or contains an invalid
UUID, the codec assigns a fresh BID on the next compile. The old BID is lost and any
existing cross-references must re-resolve.

### 7.2 `__noet_id__` — User-authored explicit id

Authors may add a `__noet_id__` column to provide stable semantic identifiers
independent of the title. Values are processed through `to_anchor()` before storage
(lowercased, spaces → hyphens, punctuation stripped). When present and non-empty, this
value is used as `BeliefNode::id` instead of the positional id assigned automatically
at parse time (see §7.4).

`__noet_id__` is never auto-created by the codec — adding it is an explicit author
choice. Use it when title uniqueness cannot be guaranteed or when you want
human-readable cross-reference IDs that survive title renames.

### 7.3 `__noet_relation_{col}__` — Relation bref write-back

For each `role: relation` column, the codec injects a companion hidden column named
`__noet_relation_{ir_key}__` (where `ir_key` is the `to_anchor()`-normalized form of
the column header). This column stores the **resolved bref** for each semicolon-separated
cell value from the corresponding relation column.

Example: a relation column `"Implements"` with two resolved references produces a
companion column `__noet_relation_implements__` containing `"abc123def456;def456abc123"`.

**Why this matters**: on the next compile, the codec reads these brefs back and pushes
them as `NodeKey::Bref` edges *alongside* the human-readable text. This gives stable
cross-shard resolution even if the target node's title or id changes — the bref is
authoritative. It eliminates the need for the full derive-from-text heuristic on
subsequent compiles.

These columns are:
- Injected automatically by `--write` after `inject_context` resolves edge targets.
- Hidden in the spreadsheet view.
- Never authored manually.
- Silently skipped when the companion relation column cell is empty.

### 7.4 Row ids — positional stable ids

Every row node receives an explicit `id` at parse time, even before a `--write` pass:

```
{workbook_prefix}-{tab_slug}-{row_number}
```

For example, a workbook file whose stem is `widget-project-requirements`, tab
`Functional Requirements`, row 1 produces:

```
widget-project-requirements-functional-requirements-1
```

This makes every row addressable by a deterministic semantic key for
cross-references (`id://widget-project-requirements-functional-requirements-1`).
`GraphBuilder` detects collisions (two rows resolving to the same id) and adjusts
the id by appending a disambiguating suffix.

When a `__noet_id__` cell is non-empty, its `to_anchor()`-processed value replaces the
positional id entirely.

### 7.5 What triggers a write-back pass

`generate_source_bytes()` runs (producing a new workbook file on disk) when either:

- **New BIDs were assigned**: at least one row node received a BID for the first time
  (`row_bids` is non-empty). This is the normal first-compile trigger.
- **Node state changed**: `inject_context` detected that an upstream relation was
  resolved (target node's title became known), or an explicit `__noet_id__` field was
  updated. The `updated` flag tracks this.

When `updated` is true, the write-back loop additionally rewrites text cell values with
the stored raw Markdown string, keeping the source file human-readable and ensuring
resolved link titles persist across compile runs.

Write-back only happens when `noet compile --write` is passed (or `noet parse`).
Read-only invocations do not modify the file.

### 7.6 Hidden column behaviour

- Hidden columns do not appear in the normal spreadsheet view but are fully present in
  the file.
- `calamine` reads hidden columns transparently — no special handling is needed.
- Authors who need to inspect or edit reserved column values can unhide them manually.
  noet will re-hide them on the next `--write` pass.

---

## 8. Formula Cells

The codec reads **computed values** of formula cells, not the formula expressions
themselves. This is consistent with how pandas, DuckDB, and other data tools read xlsx.

| Cell content                              | What the codec sees       |
|-------------------------------------------|---------------------------|
| `=SUM(A1:A3)` (evaluates to 42)           | `42`                      |
| `=VLOOKUP(...)` (evaluates to `"Safety"`) | `"Safety"`                |
| `=IF(...)` (evaluates to `#N/A` error)    | empty (treated as blank)  |
| `=TODAY()` (date value)                   | ISO date string           |

Formula errors (`#N/A`, `#VALUE!`, `#REF!`, etc.) are treated as empty cells. A
`title`-role cell with a formula error triggers the auto-generated title fallback and a
`Warning` diagnostic.

---

## 9. Validation and Diagnostics

All diagnostics are surfaced through `noet compile`'s standard output. Warnings never
abort the build — nodes are always emitted with best-effort data.

| Situation | Severity | Behaviour |
|---|---|---|
| No `index` tab | — | File treated as binary asset, no nodes, no message |
| `index` tab A1 unparseable (not YAML/JSON/TOML) | Error | File treated as binary asset |
| `index` tab A1 valid syntax but missing `title` field | Error | File treated as binary asset |
| Tab declared in schema but absent from workbook | Warning | Tab container node emitted, no rows |
| Column declared in schema but absent from header row | Warning | Column skipped; other columns parsed normally |
| No `title`-role column determinable for a tab | Warning | Rows use auto-generated titles `<Tab>:<row>` |
| Empty data row (all cells blank) | — | Row silently skipped |
| Data row missing `title`-role value | Warning | Row emitted with auto-generated title |
| `text_template` row with no meaningful content and non-empty data | Warning | Row skipped; check placeholder spelling |
| `relation` cell containing unresolvable reference | Warning | Edge omitted; row node still emitted |
| Unrecognised `__noet_<x>__` column header | Warning | Treated as `payload` |
| Tab exceeds `u16::MAX` (65 535) rows | Warning | Entire tab treated as opaque; CSV asset exported |
| Explicit schema column present in header but empty in every data row | Warning | Likely a column rename or stale schema entry; rows still emitted |

> **Note — implicit `id` from title**: Every row node that has no explicit `id` field
> (neither from `__noet_id__` nor a user-authored value) receives a positional id at
> parse time (§7.4). Two row nodes with identical auto-generated ids in the same network
> will have the second one disambiguated by `GraphBuilder`. Use `__noet_id__` to assign
> stable, distinct semantic IDs when collision-free addressing matters.

### Reading diagnostics

Run with `RUST_LOG=warn` to see all warnings:

```sh
RUST_LOG=warn noet compile --write
```

Each warning includes the file path, tab name, and row number where applicable:

```
[warn] requirements.xlsx: tab 'Functional Requirements' row 3: missing title value
       — using auto-generated title 'Functional Requirements:3'
[warn] requirements.xlsx: tab 'Components' row 7: unresolvable relation 'bad-bref'
       in column 'Implements' — edge omitted
[warn] requirements.xlsx: tab 'Raw Data' has 70000 data rows, exceeding the
       architectural limit of 65535 (u16::MAX). Treating as opaque tab — no row
       nodes emitted. Full dataset exported to .noet/derived/.
```

---

## 10. Viewer Behaviour

Compiled workbook HTML files use `xlsx-tabs.js`, a self-contained IIFE loaded on the
workbook page, to drive interactive tab switching and relation resolution.

### 10.1 Tab switching

Each schema-declared tab is rendered as a `<section id="{tab-node-id}" class="xlsx-tab">`
block. `xlsx-tabs.js` shows exactly one tab at a time by toggling `is-active` CSS
classes on tab sections and nav links.

**Tabulator tables are lazy-initialised**: a Tabulator instance is created for a tab
only on its first activation. This keeps page load fast for workbooks with many tabs.
All column definitions and row data are embedded in the page as a JSON block
(`<script type="application/json" id="noet-xlsx-data">`) and read at runtime — no
additional network requests are needed.

### 10.2 Hash routing

The viewer supports two URL forms:

| URL form | Effect |
|---|---|
| `workbook.html#tab-id` | Opens the workbook page and activates the named tab |
| `workbook.html#row-id` | Activates the containing tab and scrolls to the row element |

In SPA mode (hash-routed): `/#/workbook.html#tab-id` and `/#/workbook.html#row-id`
are handled equivalently. Tab links update `window.location.hash` via
`history.pushState` without triggering a full navigation.

When clicking a row in the table, `xlsx-tabs.js` updates the hash to the row's
positional id (`widget-project-requirements-functional-requirements-1`), making the
URL bookmarkable and shareable.

### 10.3 Relation links — two-click pattern

Relation cells are rendered as clickable elements. The viewer uses a **two-click
pattern** for navigation:

1. **First click** on a relation link → opens the metadata panel (sidebar) for the
   target node, showing its title, kind, relations, and payload without leaving the
   current page.
2. **Second click** on the same link → navigates to the target node's document page
   (if one exists).

This pattern is consistent with how the rest of the noet viewer handles internal links.
The `handleNodeLinkClick` callback from `viewer.js` drives it; `xlsx-tabs.js` registers
no global state — it reads callbacks from `window.__xlsxCbs` at click time so late
binding works correctly.

### 10.4 Unresolved relation keys

When a relation cell value cannot be resolved to a corpus node (unknown bref, missing
shard, parse-time unresolvable id), the item is rendered as:

```html
<span class="xlsx-rel-unresolved" title="id:some-key">some-key</span>
```

Muted italic text, no click action. This is intentional: clicking an unresolved key
produced confusing browser behaviour in older versions (the raw `id:xxx` string was
misclassified as a URL and opened a new tab). Authors should check for unresolved keys
by looking for the `xlsx-rel-unresolved` CSS class in the rendered page.

To resolve stale relation keys: run `noet compile --write`. The write-back pass
populates `__noet_relation_{col}__` columns with resolved brefs (§7.3), which the
viewer uses as authoritative on subsequent visits.

### 10.5 Row click

Clicking anywhere on a row (outside a relation link) opens the metadata panel for
that row's node. The row's BID is stored in the `data-bid` attribute of the Tabulator
row element; the viewer reads it to call `showMetadataPanel(bid)`.

If a row has no BID (the workbook has never been built with `--write`), a console
warning is emitted and no panel opens. Run `noet compile --write` to stabilize BIDs
before sharing the workbook.

---

## 11. Worked Example

A generic requirements workbook with two schema-declared tabs and one opaque tab.

### Workbook tab layout

```
[ index ] [ Functional Requirements ] [ Components ] [ Changelog ]
```

### `index` tab, cell A1

```yaml
title: "Widget Project Requirements"
id: "widget-req"
tabs:
  - name: "Functional Requirements"
    text_template: "{{Description}}\n\n**Rationale**: {{Rationale}}"
    schema:
      - col: "Implements"
        role: relation
        weight: pragmatic
      - col: "See Also"
        role: relation
        weight: epistemic
      - col: "Priority"
        role: payload
  - name: "Components"
    schema:
      - col: "Description"
        role: text
      - col: "Subsystem"
        role: payload
```

### `Functional Requirements` tab (as authored)

| Title | Description | Rationale | Implements | See Also | Priority |
|---|---|---|---|---|---|
| Startup Init | The system shall initialise at power-on. | Required for safe operation. | | | HIGH |
| Command Response | The system shall respond within 100 ms. | Real-time constraint. | abc123def456 | | HIGH |
| Status Reporting | The system shall report status at 1 Hz. | Observability requirement. | abc123def456 | def456abc123 | MED |

Notes:
- `Title` is not listed in `schema` — it matches the conventional name `"title"`
  (case-insensitive) and receives `role: title` automatically (layer-2 match, §5.2).
- `Description` and `Rationale` are referenced by `text_template`; they are not listed
  in `schema`. The composed text is stored as `node.payload["text"]` and indexed.
- `Implements` brefs point to design nodes elsewhere in the corpus.
- `Priority` uses `role: payload` — stored but not search-indexed.

### After `noet compile --write`

The codec injects hidden columns. Authors do not see these in normal spreadsheet view:

| … | `__noet_bid__` | `__noet_relation_implements__` | `__noet_relation_see_also__` |
|---|---|---|---|
| Startup Init row | `550e8400-…` | *(empty)* | *(empty)* |
| Command Response row | `6ba7b810-…` | `abc123def456` | *(empty)* |
| Status Reporting row | `6ba7b811-…` | `abc123def456` | `def456abc123` |

On subsequent compiles the brefs are read back and pushed as `NodeKey::Bref` edges,
giving stable resolution independent of title or id changes in the target nodes.

### `Changelog` tab

This tab records edit history in a team-specific format. By omitting it from the
`index` schema, noet treats it as an opaque tab: one container node is emitted and
the full tab content is available at `.noet/derived/requirements__Changelog.csv`.

### Resulting node hierarchy

```
requirements.xlsx                      (Document, heading=2, id="widget-req")
  ├── Functional Requirements           (Symbol, heading=3)
  │     ├── Startup Init                (Symbol, heading=4)
  │     │     id: "widget-req-functional-requirements-1"
  │     │     payload.text: "The system shall initialise at power-on.\n\n**Rationale**: Required for safe operation."
  │     ├── Command Response            (Symbol, heading=4)
  │     │     id: "widget-req-functional-requirements-2"
  │     │     upstream: abc123def456 [Pragmatic]
  │     └── Status Reporting            (Symbol, heading=4)
  │           id: "widget-req-functional-requirements-3"
  │           upstream: abc123def456 [Pragmatic]
  │           upstream: def456abc123 [Epistemic]
  ├── Components                        (Symbol, heading=3)
  │     └── (row nodes…)
  └── Changelog                         (Symbol, heading=3, opaque)
        upstream → .noet/derived/requirements__Changelog.csv [Epistemic]
```

---

## 12. Multi-Format Support

Cell A1 may contain the schema in YAML, JSON, or TOML. The codec tries YAML first
(most compact for nested lists), then JSON, then TOML.

**TOML equivalent** of the minimal example:

```toml
title = "Widget Project Requirements"

[[tabs]]
name = "Functional Requirements"

[[tabs.schema]]
col = "Description"
role = "text"

[[tabs.schema]]
col = "Implements"
role = "relation"
```

**JSON equivalent**:

```json
{
  "title": "Widget Project Requirements",
  "tabs": [
    {
      "name": "Functional Requirements",
      "schema": [
        { "col": "Description", "role": "text" },
        { "col": "Implements", "role": "relation" }
      ]
    }
  ]
}
```

YAML is recommended for human authoring. JSON is convenient when generating the schema
programmatically from a script that inspects column headers.

---

## 13. Limitations and Known Issues

- **Cell A1 display truncation**: Excel and LibreOffice may show only the first line
  of A1 in the cell view. The full content is always stored; use the formula bar or
  press F2 to see it. For very large schemas, consider whether TOML or JSON fits more
  compactly in a single cell.

- **`text_template` supersedes `role: text`**: when `text_template` is present on a
  tab, individual `role: text` column declarations for that tab are ignored for body
  text composition. The template is the single authoritative source for `payload["text"]`.
  If you need both a composed body and individually addressable text fields, declare the
  raw columns as `role: payload` and let the template compose them.

- **Multi-text-column write-back**: when `updated = true` triggers a write-back and a
  tab has multiple `role: text` columns (no `text_template`), the first text column
  receives the composed value and secondary text columns are blanked. This is because
  the codec stores the joined string as a single value (`\n\n`-joined) and cannot
  reliably reverse the join. Use `text_template` with individual columns declared as
  `role: payload` if you need them preserved verbatim.

- **ODS formula values**: LibreOffice Calc stores formula cached values in ODS files;
  the codec reads them correctly. ODS files saved by older versions of LibreOffice may
  omit cached values for some formula types — those cells are treated as empty.

- **Tab name case sensitivity**: `name: "Functional Requirements"` will not match a
  tab named `functional requirements`. Spellings must match exactly. This is deliberate
  — spreadsheet tab names are case-sensitive in all major tools.

- **Maximum rows**: 65 535 data rows per tab (`u16::MAX`). When exceeded, the entire
  tab is treated as opaque (no row nodes; CSV asset in `.noet/derived/`). This limit
  comes from `PathMap`'s `u16` sort-key space, not from the xlsx format.

- **Binary assets**: `.xlsx` files with no `index` tab are treated as binary assets —
  they appear in the asset namespace with a content hash but produce no document nodes.
  Add an `index` tab to opt in to structured parsing.

- **Viewer BID requirement**: the metadata panel and row-click navigation require that
  each row has a BID injected via `--write`. Workbooks that have never been compiled
  with `--write` will show a console warning on row click and no panel will open.
  Run `noet compile --write` at least once before sharing a compiled workbook.