# Issue 52: Network Index Page Content Merge

**Priority**: HIGH
**Estimated Effort**: 2 days (RELATIVE COMPARISON ONLY)
**Dependencies**: Requires working `NetworkCodec` (complete), Blocks Issue 13 (full HTML CLI output)

## Summary

`NetworkCodec::generate_deferred_html` currently generates the child-document listing from
scratch, discarding authored prose in `index.md`. We need to merge the two: the human-written
body of `index.md` and the auto-generated directory listing. This is the first use of
quasi-dynamic content injection in the render pipeline.

## Goals

- Authored prose in `index.md` is always preserved in the rendered `index.html`
- Auto-generated child listing is inserted at the end of the body when no explicit placement
  marker is present
- A placement marker `<!-- network-children -->` in `index.md` controls where the listing is
  injected when present
- The mechanism does **not** corrupt `generate_source` round-trips — source content must remain
  clean
- The marker protocol becomes a generalizable extension point for other quasi-dynamic content
  (e.g. backlink lists, tag clouds) in future codecs

## Architecture

### Current flow (compiler perspective)

```
parse_next()
  → codec.generate_html()           # immediate: writes prose-only HTML body to disk via
                                    #   write_fragment (step 7a)
  → deferred_html.insert(path)      # queued for later (step 7b, because should_defer == true)

finalize_html()
  → generate_deferred_html(ctx)
      → generate_html_for_path()
          → codec_factory()         # fresh codec instance — no parsed state
          → codec.generate_deferred_html(ctx, output_path)
              # currently: builds listing-only HTML from scratch, ignores prose
```

The immediate `generate_html()` write at step 7a is overwritten by the deferred phase.
For `NetworkCodec`, only the deferred output currently survives on disk, and it contains only
the listing — no prose.

### Target flow

```
parse_next()
  → codec.generate_html()           # immediate: renders prose + sentinel, written to disk
                                    #   via write_fragment as today
  → deferred_html.insert(path)      # queued as before

finalize_html()
  → generate_deferred_html(ctx)
      → generate_html_for_path()
          → codec_factory()         # fresh codec — no parsed state needed
          → compute output_path     # html_output_dir / base_dir / "index.html"
          → codec.generate_deferred_html(ctx, &output_path)
              # NetworkCodec: reads existing file, replaces sentinel, writes back → Ok(None)
              # other codecs: return Ok(Some((filename, body))) → compiler calls write_fragment
```

The sentinel lives in the **on-disk HTML file**, written there by the immediate phase inside
`{{BODY}}`. The deferred codec is fresh and stateless — it reads the existing file, splices in
the listing, and writes back. No parsed event state required.

### Trait signature change

`generate_deferred_html` gains an `existing_html_path` parameter and changes its return type:

```rust
fn generate_deferred_html(
    &self,
    ctx: &BeliefContext<'_>,
    existing_html_path: &Path,
) -> Result<Option<(String, String)>, BuildonomyError> {
    Ok(None)
}
```

Return semantics:

- `Ok(None)` — codec handled the write itself (in-place replacement). Compiler does nothing
  further. This is also the correct return value when there is nothing to write.
- `Ok(Some((filename, body)))` — compiler calls `write_fragment` as normal (used by codecs that
  generate output from scratch without needing to modify an existing file)
- `Err(_)` — generation failed

The default implementation returns `Ok(None)` (no deferred generation needed, nothing to write).
This replaces the current default of `Ok(vec![])`.

### Sentinel and marker convention

Two constants in `network.rs`:

```rust
/// Collision-safe placeholder emitted into the HTML body by generate_html().
/// Survives write_fragment's Layout::Simple template wrapping because it sits
/// inside {{BODY}}. Always replaced by generate_deferred_html before the file
/// is considered complete. This string is reserved and must not appear in user content.
const NETWORK_CHILDREN_SENTINEL: &str = "<!--@@noet-network-children@@-->";

/// Author-facing placement marker. Write this raw HTML comment anywhere in the
/// index.md body to control where the auto-generated child listing is injected.
/// If absent, the listing is appended at the end of the body.
const NETWORK_CHILDREN_MARKER: &str = "<!-- network-children -->";
```

`generate_html()` placement logic (event iterator only — **no mutation of `current_events`**):

1. Scan `current_events.first().1` for a `MdEvent::Html` whose trimmed value equals
   `NETWORK_CHILDREN_MARKER`.
2. If found: substitute that event with `MdEvent::Html(NETWORK_CHILDREN_SENTINEL.into())` in
   the rendering iterator only.
3. If not found: render all events normally, then append `NETWORK_CHILDREN_SENTINEL` to the
   resulting HTML string.

`NetworkCodec::generate_deferred_html(ctx, existing_html_path)` logic:

1. If `existing_html_path` exists on disk: read its contents (full Layout::Simple-wrapped HTML).
2. If it does not exist: start from an empty body string (fallback — immediate phase was skipped).
3. Generate listing HTML from `ctx` (child edges, sorted by `WEIGHT_SORT_KEY`), including
   empty-state message if no children.
4. If `existing_html_path` exists: replace `NETWORK_CHILDREN_SENTINEL` with listing HTML in the
   full file content and write back to `existing_html_path` directly. Return `Ok(None)`.
5. If `existing_html_path` does not exist: return `Ok(Some(("index.html".to_string(),
   listing_html)))` so the compiler writes it via `write_fragment` as a fallback.

### Compiler change in `generate_html_for_path`

`generate_html_for_path` must compute `existing_html_path` before calling the codec:

1. Derive `output_path` using the same `base_dir` + filename logic already present in the
   function, joined with `html_output_dir / "pages"` (matching `write_fragment`'s layout).
2. Pass `&output_path` to `codec.generate_deferred_html(ctx, &output_path)`.
3. On `Ok(None)`: do nothing — codec already wrote the file.
4. On `Ok(Some((filename, body)))`: call `write_fragment` as today.

### Source round-trip safety

`generate_source()` reads from `current_events` and is never called in the deferred phase.
The sentinel appears only in the rendered HTML string returned by `generate_html()`, never in
the event queue. `generate_source()` is therefore unaffected.

## Implementation Steps

1. Add sentinel/marker constants to `network.rs` (0.25 days)
   - [ ] Define `NETWORK_CHILDREN_SENTINEL` and `NETWORK_CHILDREN_MARKER` with doc comments
         explaining their contract and reserved status

2. Update `NetworkCodec::generate_html` to emit the sentinel (0.5 days)
   - [ ] Scan `current_events.first().1` for the marker event index
   - [ ] Build the rendering iterator; if marker found, substitute that event with
         `MdEvent::Html(NETWORK_CHILDREN_SENTINEL.into())` — do **not** mutate `current_events`
   - [ ] If no marker found, append `NETWORK_CHILDREN_SENTINEL` to the rendered HTML string
   - [ ] Confirm `generate_source()` output is unchanged (existing round-trip tests pass)

3. Update `DocCodec::generate_deferred_html` trait signature (0.25 days)
   - [ ] Change signature to `(&self, ctx: &BeliefContext<'_>, existing_html_path: &Path) -> Result<Option<(String, String)>, BuildonomyError>`
   - [ ] Update default implementation to return `Ok(None)`
   - [ ] Update all existing `generate_deferred_html` implementations to accept the new signature

4. Implement `NetworkCodec::generate_deferred_html` merge logic (0.5 days)
   - [ ] If `existing_html_path` exists: read full file, generate listing HTML, replace sentinel,
         write back, return `Ok(None)`
   - [ ] If `existing_html_path` does not exist: generate listing HTML, return
         `Ok(Some(("index.html".to_string(), listing_html)))` as fallback
   - [ ] If sentinel is absent from existing file (e.g. older generated file on disk): append
         listing to file content before writing back

5. Update `generate_html_for_path` in `compiler.rs` (0.25 days)
   - [ ] Compute `output_path` as `html_output_dir / "pages" / base_dir / filename`
   - [ ] Pass `&output_path` to `codec.generate_deferred_html`
   - [ ] On `Ok(None)`: no further action
   - [ ] On `Ok(Some(...))`: call `write_fragment` as today

6. Tests (0.25 days)
   - [ ] `test_network_index_no_marker`: prose in `index.md`, no marker; listing appended after
         prose in rendered HTML
   - [ ] `test_network_index_with_marker`: `<!-- network-children -->` mid-body; prose split
         around listing at marker position
   - [ ] `test_network_index_source_roundtrip`: `generate_source()` output equals original
         `index.md` content; marker line unchanged, no sentinel present
   - [ ] `test_sentinel_not_in_final_html`: sentinel string absent from all on-disk HTML after
         deferred phase completes
   - [ ] Extend `tests/network_1/subnet1/index.md` fixture (or add a sibling) to include the
         marker for integration coverage

## Testing Requirements

- Round-trip: `generate_source()` output must not contain `NETWORK_CHILDREN_SENTINEL`
- Final HTML: `NETWORK_CHILDREN_SENTINEL` must not appear in any on-disk HTML after the deferred
  phase completes
- Marker-absent: prose always appears before listing in final HTML
- Marker-present: prose-before-marker, listing, prose-after-marker all present
- Empty network: empty-state message replaces sentinel; prose still preserved
- Fallback: when `existing_html_path` does not exist, listing-only output is written correctly

## Success Criteria

- [ ] `tests/network_1` integration test renders `index.html` files containing both authored
      prose from `index.md` and the auto-generated child listing
- [ ] `generate_source()` round-trip tests pass without modification
- [ ] No sentinel string appears in any on-disk HTML after deferred phase (asserted in tests)
- [ ] `NETWORK_CHILDREN_MARKER` convention documented in code comment on the constant

## Risks

- **Sentinel collision**: a document containing the sentinel string verbatim would have content
  replaced. → **Mitigation**: double `@@` wrapping makes accidental collision effectively
  impossible; document as reserved in `NETWORK_CHILDREN_SENTINEL`'s comment.

- **Missing immediate output**: if `html_output_dir` was not configured at parse time, the
  immediate phase never ran and `existing_html_path` will not exist. The fallback path in step 4
  handles this by returning a fragment for the compiler to write via `write_fragment`.

- **Sentinel absent from existing file**: if the existing file has no sentinel, that means
  `generate_html` intentionally did not emit one (author config, future opt-out mechanism, or
  stale file from an older build). The deferred phase must respect this and do nothing.
  → **Mitigation**: `tracing::info!` that the sentinel was not found and return `Ok(None)`.

## Open Questions

- **Default marker placement**: when no marker is present, listing always appends after all
  rendered events including any trailing `---` thematic break. Confirm this is desired order.

- **Marker in frontmatter vs. body**: current proposal uses a raw HTML comment in the Markdown
  body. A frontmatter key (e.g. `children_placement = "inline"`) is an alternative. Deferred —
  raw HTML comment is simpler and author-visible.

## Future Enhancements

- **Dynamic `should_defer`**: `should_defer()` currently returns a static `true` for
  `NetworkCodec`. Now that `generate_html()` controls sentinel emission, `should_defer()` could
  be made dynamic — returning `true` only when a sentinel was actually emitted (i.e. content was
  generated and a placement point exists). This would allow the compiler to skip the deferred
  queue entirely for network nodes that have no authored content and no children, avoiding a
  redundant deferred pass. Low priority; implement when profiling shows it matters.