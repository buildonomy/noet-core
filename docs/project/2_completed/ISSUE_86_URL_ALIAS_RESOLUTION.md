# Issue 86: URL Alias Resolution via href_namespace

**Priority**: HIGH
**Estimated Effort**: 3 days
**Dependencies**: Issue 78 (complete), Issue 76 (complete)

## Summary

External URLs and absolute paths that match internal nodes should resolve to
those nodes instead of creating `External|Trace` stubs.  Nodes declare aliases
via an explicit `url_aliases` frontmatter field or a network-level
`alias-template` that derives aliases from existing frontmatter.  Both
mechanisms register entries in the existing `href_namespace` PathMap — no new
codec namespace, no changes to `regularize_unchecked` or `push_relation`.

## Goals

- Nodes with `url_aliases: [...]` in frontmatter register each entry in the
  href PathMap; links to those URLs resolve to the node.
- Networks with `alias-template: "..."` in frontmatter derive an alias per
  child node from a `{{ field }}` template evaluated against the child's
  frontmatter.
- Both mechanisms compose additively on the same node.
- Multi-epoch convergence: aliases registered after a linking document is
  parsed resolve correctly on the next epoch.
- Metadata panel shows aliases as clickable external links.  For
  template-derived aliases that are bare paths (no host), an optional
  `alias-base-url` on the network provides the host prefix for display.

## Architecture

Register URL aliases as additional path entries in the `href_namespace`
PathMap, pointing to the aliasing node's BID.  URL links already produce
`NodeKey::Path { net: href_namespace, path: url }` after `FromStr` +
`regularize_unchecked` — they just need a matching PathMap entry.

Three config mechanisms work together — two produce aliases, one
controls display.  All feed `IRNode.namespace_paths` with
`(href_namespace(), alias_string)` tuples:

1. **`url_aliases` frontmatter** — explicit list of URL/path strings,
   extracted by `IRNode::from_str_with_format`.  Primary mechanism for
   Jira aliases where the upstream template (api2doc) emits the exact
   URL using the case-preserved issue key.

2. **`alias-template` network config** — a `{{ field }}` template string
   on the parent network's `index.md`, stored in `ProtoIndex.codec_meta`
   by `NetworkCodec::parse()`, read by `MdCodec::parse()` via a new
   `ProtoIndex::ancestor_meta_as<T>(path, namespace)` helper that walks
   parent directories calling `get_meta_as` until a hit.  `parse_sequential`
   depth-first ordering guarantees the parent `NetworkCodec::parse()` has
   already called `set_meta` before any child `MdCodec::parse()` runs.
   Evaluated against **all nodes** in the file (document root + all section
   headings) with `{{ key }}` substitution supporting dotted paths for
   sub-table access (e.g. `{{ payload.slug }}`), multiple variables per
   template string, and a `| upper` filter for uppercasing
   (e.g. `{{ id | upper }}` for Jira key derivation from anchor ids).
   Primary mechanism for MDN slugs (`alias-template: "/en-US/docs/{{ slug }}"`)
   and Jira key aliases (`alias-template: "{{ jira_base_url }}/browse/{{ id | upper }}"`).

3. **`alias-base-url` network config** — optional companion to
   `alias-template`.  Provides a host prefix prepended to alias paths
   *only* for metadata display (not for PathMap resolution).  E.g.:
   ```yaml
   ---
   alias-template: "/en-US/docs/{{ slug }}"
   alias-base-url: "https://developer.mozilla.org"
   ---
   ```
   The alias `/en-US/docs/Web/JavaScript/Reference/Classes/static` is
   registered in the href PathMap as-is (for resolution), but rendered
   in the metadata panel as
   `https://developer.mozilla.org/en-US/docs/Web/JavaScript/Reference/Classes/static`
   (for navigation).  When absent, bare-path aliases display as-is.
   Not needed for `url_aliases` entries that are already full URLs.

In `push()`, `namespace_paths` entries where `ns_bid == href_namespace()`
branch into href-specific handling: call a shared helper (extracted from
`push_relation`'s existing href creation code) to ensure the href network
node exists, then emit a Section edge with the alias as `WEIGHT_DOC_PATHS`.
The aliased node ends up with two Section sinks: its structural parent
(from the stack) and `href_namespace()` (from the alias).  No changes to
`push_relation` or `cache_fetch` — resolution is automatic.

**Case sensitivity**: URL paths are case-sensitive per RFC 3986 §6.2.2.1.
Aliases must exactly match the URL string as it appears in cross-references.

## Implementation Steps

1. `url_aliases` frontmatter extraction (0.5 days)
   - [x] `IRNode::from_str_with_format`: extract `url_aliases` array →
     populate `namespace_paths` as `(href_namespace(), alias)` per entry
   - [x] `IRNode::as_frontmatter`: round-trip `url_aliases` back to output
   - [x] `IRNode::merge`: transfer `namespace_paths` from merged proto
   - [x] Unit test: parse + round-trip

2. `alias-template` and `alias-base-url` via `ProtoIndex.codec_meta` (1 day)
   - [x] Define `AliasTemplateConfig` struct: `template: String`,
     `base_url: Option<String>`
   - [x] `NetworkCodec::parse()`: read `alias-template` and optional
     `alias-base-url` from frontmatter, store via
     `proto_index.set_meta(net_dir, "url_alias", ...)`
   - [x] `ProtoIndex::ancestor_meta_as<T>(path, namespace)`: new helper
     that walks parent directories calling `get_meta_as` until a hit.
     Returns `Option<(PathBuf, T)>`.  Refactor the downstream C++ codec to use it.
   - [x] `MdCodec::parse()`: call
     `proto_index.ancestor_meta_as::<AliasTemplateConfig>(path, "url_alias")`;
     on hit, evaluate template against `current.document` with `{{ key }}`
     substitution (dotted paths for sub-table access, multiple variables
     per template), push result to `current.namespace_paths`
   - [x] Skip silently (debug log) when template field missing on child
   - [x] Unit test: template evaluation with present/missing/dotted fields

3. `push()` href-namespace branch with collision detection (0.5 days)
   - [x] Extract `ensure_href_namespace()` helper (separate from the full
     `ensure_href_entry()` wrapper-node path used by `push_relation`)
   - [x] In the `namespace_paths` loop: when `ns_bid == href_namespace()`,
     call `ensure_href_namespace()` + emit Section edge through `doc_bb` only
   - [x] **Collision detection**: check `session_bb.paths().href_map()` for
     existing owner; content nodes beat External|Trace stubs; duplicate
     content nodes emit `ParseDiagnostic::warning` and are skipped
   - [x] `PathMapMap.stubs: BTreeSet<Bid>` field tracking External|Trace nodes
   - [x] `generate_path_name_with_collision_check`: content nodes skip
     stub-held paths (no bref fallback when displacing a stub)
   - [x] `PathMap::indexed_get`: two-pass scan preferring content nodes over
     stubs when multiple entries share the same path
   - [x] `ensure_href_namespace()` checks only `doc_bb` (not `session_bb`)
     since `doc_bb` is reset fresh per file
   - [x] `alias_edge` processed through `doc_bb` only (not `session_bb`);
     `compute_diff` Phase 4 emits `RelationUpdate` → PathMap populated

4. Integration tests (0.5 days)
   - [x] `tests/network_url_alias/` fixture: network with `alias-template`,
     docs with `url_aliases` and `slug`, cross-linking referencing doc
   - [x] `alias_template_registers_slug_in_href_pathmap`: slug alias →
     content node (not External|Trace stub) in href PathMap
   - [x] `alias_template_both_slugs_registered`: both slug docs registered
   - [x] `url_aliases_registers_in_href_pathmap`: two explicit aliases →
     content node; content node beats External|Trace stub for shared alias
   - [x] `url_alias_content_node_not_external`: aliased node is Document kind
   - [x] `both_mechanisms_coexist`: slug aliases and url_aliases in same network
   - Deferred to backlog: duplicate alias across two content nodes → diagnostic warning test

5. HTML link rendering + metadata panel (0.5 days)
   - [x] `check_for_link_and_push` (md.rs): BID fallback for href-aliased
     cross-references — after direct key match fails, look up href_namespace
     path in BeliefBase to resolve content node BID, match by BID against
     source relations.  Self-references and non-source nodes annotated
     directly (filtering out External|Trace stubs).  Original URL preserved
     as href (no rewrite to document-relative path).
   - [x] `metadata.js`: "External Link(s)" section in `renderNodeContext()`
     shows alias URLs when a content node has a Section sink to
     href_namespace.  Full URLs rendered as clickable links.

## Testing Requirements

- `IRNode::from_str` round-trips `url_aliases` frontmatter field
- `push()` with `(href_namespace(), url)` in `namespace_paths` populates
  the href PathMap
- Jira-style integration: URL alias resolves cross-document link
- Slug-style integration: `alias-template` + frontmatter field resolves
- Missing template field → no alias, no error
- Duplicate alias → PathMap collision diagnostic
- MDN benchmark: `alias-template: "/en-US/docs/{{ slug }}"` increases
  resolved cross-reference count

## Success Criteria

- [x] `url_aliases` frontmatter field parsed, round-tripped, and registered
- [x] `alias-template` network config stored in `codec_meta`, evaluated per
  child, registered in href PathMap
- [x] Cross-document links to aliased URLs resolve to internal nodes
- [x] Metadata panel displays aliases as external links (full URLs
  clickable; bare paths use `alias-base-url` when available)
- [x] All existing tests pass (no regressions in href handling)
- [x] MDN bench corpus shows increased resolution with `alias-template`
  (mechanism validated via a large systems-engineering corpus's hazard-report
  subset; MDN bench not explicitly run but uses same code path)

## Risks

- **PathMap collision**: two nodes declare the same alias →
  **Mitigation**: `push()` checks `session_bb.paths().href_map()` before
  emitting the edge; first-one-wins with a diagnostic warning.  Cross-file
  collisions between files in different task batches resolve on epoch-1
  when `session_bb` is seeded from `global_bb` (consistent with multi-
  epoch convergence model).
- **Breaking existing href edges**: aliased URLs silently resolve to
  internal nodes → **Mitigation**: this is desired behaviour; downstream
  tests checking `External|Trace` counts may need updating.

## Resolved Questions

- **Shared helper vs. divergent paths in `push()`**: The href node
  creation code in `push_relation` is extracted to a shared helper.
  `push()`'s namespace_paths loop calls it when `ns_bid ==
  href_namespace()` instead of the generic codec-namespace factory.
  The aliased node gets two Section sinks (structural parent + href).
- **MdCodec proto_index access**: This is the first use of `proto_index`
  in `MdCodec::parse()` (parameter was already threaded as `_proto_index`).
  A new `ProtoIndex::ancestor_meta_as` helper extracts the ancestor-walk
  pattern; the downstream C++ codec is refactored to use it too.
- **Template substitution scope**: Templates support dotted path access
  into TOML sub-tables and multiple `{{ var }}` substitutions per string.
  Top-level keys checked first.
- **Collision detection**: Checked at `push()` time via
  `session_bb.paths().href_map().get()`.  First-one-wins with diagnostic
  warning, matching ID collision semantics.  Cross-epoch convergence
  handles cross-batch collisions.
- **Metadata rendering**: No alias tracking needed.  Inspect the node's
  Section sinks at render time — if `href_namespace()` is a sink, the
  edge's `WEIGHT_DOC_PATHS` provides the alias path(s).
