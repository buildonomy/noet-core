# Issue 51: Author Feedback — Compiler Diagnostics Pipeline

**Priority**: HIGH
**Estimated Effort**: 2 days (RELATIVE COMPARISON ONLY)
**Dependencies**: None (self-contained within existing codec stack)
**Blocks**: Issue 11 (LSP diagnostics will consume this pipeline)

## Summary

Authors currently get no feedback when a link in a document fails to resolve — the compiler
silently logs a `tracing::info!` and leaves the link unchanged. This issue promotes those silent
tracing calls into structured `ParseDiagnostic` entries that flow through `ParseResult`, giving
authors actionable, document-level error output from the CLI. The compiler already knows when a
parse round is final; it is the right place to decide that an `UnresolvedReference` has become a
permanent author-visible failure.

## Goals

1. Convert `tracing::info!/warn!` calls in `check_for_link_and_push` (and adjacent codec paths)
   into `ParseDiagnostic::UnresolvedReference` entries so they travel through the existing
   diagnostic pipeline — no new variant needed.
2. In `DocumentCompiler::parse_next`, after the reparse queue stabilizes, promote lingering
   `UnresolvedReference` diagnostics to `ParseDiagnostic::Warning` with a human-readable
   message, filtering them out of the internal reparse machinery.
3. Emit a clean, human-readable diagnostic report from the CLI (`noet build`), formatted like
   compiler output (`path:line:col: [warning] message`).
4. Add tests asserting that specific link-resolution failures produce the expected diagnostics
   after compilation completes.

## Architecture

### The Gap Today

`check_for_link_and_push` in `src/codec/md.rs` calls `tracing::info!` when a link cannot be
matched to any context node. That information is never returned to the caller — `inject_context`
returns only `Result<Option<BeliefNode>, BuildonomyError>`. Meanwhile `ParseContentResult` already
carries `diagnostics: Vec<ParseDiagnostic>`, and `GraphBuilder::parse_content` already accumulates
`UnresolvedReference` entries into it from the builder layer.

The fix has two parts:

**Part A — Codec emits diagnostics for unresolved links.**
`check_for_link_and_push` already receives `events_out: &mut VecDeque<…>` as an out-parameter.
Add `diagnostics: &mut Vec<ParseDiagnostic>` the same way. When a link has no matching relation,
push an `UnresolvedReference` (reusing the existing type) instead of calling `tracing::info!`.
`inject_context` passes `&mut diagnostics` through and the caller (`GraphBuilder::parse_content`)
accumulates them into `ParseContentResult` — which already happens for builder-layer unresolved
references today.

This is a *local* signature change to `check_for_link_and_push` (a private function) and the
`inject_context` call site inside `MdCodec`. It does **not** require changing the `DocCodec`
trait.

**Part B — Compiler promotes lingering references to warnings on the final pass.**

`DocumentCompiler` already tracks `reparse_stable: bool` and `max_reparse_count`. After
`reparse_stable` is true (no more round updates) or `max_reparse_count` is hit, any
`UnresolvedReference` remaining in a `ParseResult` has definitively failed to resolve. At that
point `parse_next` (or a post-`parse_all` sweep) replaces each such entry with a
`ParseDiagnostic::Warning` carrying a formatted `path:line:col: unresolved link …` message.

This keeps `UnresolvedReference` as the compiler's internal multi-pass signal while ensuring the
final `Vec<ParseResult>` handed to the CLI contains only author-facing variants
(`Warning`, `ParseError`, `Info`).

### Data Flow

```
MdCodec::inject_context
  └─ check_for_link_and_push(…, diagnostics: &mut Vec<ParseDiagnostic>)
       └─ push UnresolvedReference   (instead of tracing::info!)
  └─ returns diagnostics via ParseContentResult

GraphBuilder::parse_content
  └─ accumulates ParseContentResult.diagnostics (already does this for builder-layer refs)

DocumentCompiler::parse_next  (per file, per pass)
  └─ reparse_queue logic unchanged — UnresolvedReference still triggers reparse

DocumentCompiler::parse_all / parse_next (final pass detection)
  └─ for each ParseResult with lingering UnresolvedReference:
       promote → ParseDiagnostic::Warning("path:line:col: unresolved link — tried [...]")

CLI (noet build)
  └─ print all Warning/ParseError diagnostics to stderr
  └─ exit non-zero only on ParseError
```

### Location Information

`UnresolvedReference` already has `reference_location: Option<(usize, usize)>`. When
`check_for_link_and_push` creates the entry, it has `link_data.range: Option<Range<usize>>`
(byte offset into the source). A small helper converts byte offset → (line, col) at that point,
populating `reference_location`. The source `&str` is available in `MdCodec::content`.

```rust
fn byte_offset_to_location(source: &str, offset: usize) -> (usize, usize) {
    let clamped = offset.min(source.len());
    let before = &source[..clamped];
    let line = before.chars().filter(|&c| c == '\n').count() + 1;
    let col = before.rfind('\n').map(|i| clamped - i - 1).unwrap_or(clamped) + 1;
    (line, col)
}
```

## Implementation Steps

### 1. Add byte-offset → (line, col) utility (0.25 days)
- [ ] Add `pub fn byte_offset_to_location(source: &str, offset: usize) -> (usize, usize)` in
      `src/codec/diagnostic.rs`
- [ ] Unit test: start-of-line, mid-line, multi-line, offset == len, offset > len inputs

### 2. Thread diagnostics out-parameter through `check_for_link_and_push` (0.5 days)
- [ ] Add `diagnostics: &mut Vec<ParseDiagnostic>` parameter to the private function
      `check_for_link_and_push` in `src/codec/md.rs`
- [ ] Replace the `tracing::info!` unresolved-link call (~line 539) with:
  ```
  diagnostics.push(ParseDiagnostic::UnresolvedReference(UnresolvedReference {
      self_path: doc_path.to_string(),
      other_keys: keys.clone(),
      reference_location: link_data.range.as_ref().map(|r|
          byte_offset_to_location(&source, r.start)
      ),
      ..UnresolvedReference::default()
  }));
  ```
- [ ] Update the single call site in `MdCodec::inject_context` to pass `&mut diagnostics`,
      then append those diagnostics into the `ParseContentResult` before returning
- [ ] `MdCodec` holds `self.content: String` — pass a reference for the byte→location conversion

### 3. Promote lingering references in the compiler (0.5 days)
- [ ] Add a helper `fn promote_unresolved_to_warnings(results: &mut Vec<ParseResult>)` in
      `src/codec/compiler.rs`
- [ ] After `parse_all` drains the reparse queue (stable or max-count hit), call this helper on
      the accumulated results
- [ ] Promotion logic: for each `ParseDiagnostic::UnresolvedReference(u)` in a final result,
      replace it with `ParseDiagnostic::Warning(format!(
          "{}:{}:{}: unresolved link — tried {:?}",
          u.self_path,
          u.reference_location.map_or(0, |l| l.0),
          u.reference_location.map_or(0, |l| l.1),
          u.other_keys
      ))`
- [ ] Entries that are genuine sink-dependency `UnresolvedReference`s (i.e., those with
      `is_unresolved_source() == true`) must NOT be promoted — they are compiler-internal. Only
      promote those originating from `check_for_link_and_push` (direction `Outgoing`, or
      identifiable by context).

### 4. Audit remaining `tracing::warn!` calls in `md.rs` (0.25 days)
- [ ] `parse_sections_metadata` warns on malformed frontmatter values — convert to
      `ParseDiagnostic::Warning` pushed through the diagnostics vec (same out-parameter pattern)
- [ ] `finalize` warns on section nodes with missing/invalid BID — convert to
      `ParseDiagnostic::Warning`; these are also author errors
- [ ] Keep `tracing::warn/error` calls that indicate internal codec bugs (not author input errors)

### 5. CLI diagnostic reporting (0.5 days)
- [ ] After `parse_all` returns, iterate `Vec<ParseResult>` and print all `Warning` and
      `ParseError` diagnostics to stderr in `path:line:col: severity: message` format
- [ ] Skip `UnresolvedReference` at print time (should be none after promotion, but defensively
      skip rather than crash)
- [ ] Exit code non-zero only when any `ParseError` diagnostic exists

## Testing Requirements

- `byte_offset_to_location` returns correct `(line, col)` for known inputs including edge cases
- `check_for_link_and_push` with a context missing the target produces exactly one
  `ParseDiagnostic::UnresolvedReference` in the diagnostics vec (no `tracing::info!` fired)
- End-to-end: compile a document with a broken link through `DocumentCompiler::parse_all`;
  the returned `Vec<ParseResult>` contains a `ParseDiagnostic::Warning` (promoted) with
  the correct file path and attempted keys; no `UnresolvedReference` remains
- Genuine sink `UnresolvedReference`s (cross-document links to files that exist) are NOT
  promoted to warnings — they resolve correctly across passes
- `ParseDiagnostic::Warning` for unresolved links prints in `path:line:col` format

## Success Criteria

- [ ] `noet build` on a corpus with broken links prints human-readable warnings to stderr, one
      per unresolved link, with file path and attempted keys
- [ ] No `tracing::info!` fires for author-visible link resolution failures
- [ ] `ParseResult.diagnostics` is the single authoritative record of per-document issues
- [ ] The `DocCodec` trait signature is unchanged — no downstream implementors broken
- [ ] LSP Issue 11 can consume `ParseResult.diagnostics` without further codec changes

## Risks

- **Distinguishing link-resolution failures from sink dependencies**: both use
  `UnresolvedReference`. Promotion must not convert sink dependencies (cross-document compile
  ordering) into spurious warnings. **Mitigation**: use `is_unresolved_source()` as the guard;
  only promote `Outgoing` references that originate from `check_for_link_and_push`. If the
  distinction is ambiguous, add a boolean flag `from_link_resolution: bool` to
  `UnresolvedReference` rather than a new variant.
- **`parse_sections_metadata` diagnostics path**: this function is called before
  `inject_context`, so it does not yet have a diagnostics vec. **Mitigation**: return
  `Vec<ParseDiagnostic>` from `parse_sections_metadata` alongside its existing return value,
  or collect them into a field on `MdCodec` during `parse()` and flush in `inject_context`.

## Open Questions

- Should promoted warnings go to stderr or be part of a structured output format (JSON)?
  Start with stderr; structured output can be a follow-up.
- Is `is_unresolved_source()` a reliable enough guard for promotion, or do we need an explicit
  `from_link_resolution` flag on `UnresolvedReference`? Decide during step 3 implementation
  once the actual data is visible.
