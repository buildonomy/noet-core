# SCRATCHPAD - NOT DOCUMENTATION
# AnchorPath Windows Drive-Letter Fix

## Status
**COMPLETE** — Option A (drive-letter-aware AnchorPath) implemented and all tests passing.

## What Was Fixed

### Root cause
`AnchorPath::new` treated a single ASCII-alpha character followed by `:` as a URL schema
(e.g. `"C:"` → `sch_sep = Some(1)`). This caused `filepath()` and `dir()` to strip the
drive-letter prefix, producing driveless absolute paths like `/Users/...` instead of
`C:/Users/...`. The driveless paths failed `starts_with` guards in `regularize_unchecked`,
causing spurious Generated nodes in `global_bb` that were never written to source files,
which broke `test_belief_set_builder_bid_generation_and_caching` on Windows CI.

### Fix applied (Option A — drive-letter-aware throughout)

**`src/paths/path.rs` — `AnchorPath::new`**

Added a `is_drive_letter` guard in the `sch_sep` parsing block:

```rust
let is_drive_letter = colon_idx == 1 && path.as_bytes()[0].is_ascii_alphabetic();
if is_drive_letter {
    None  // treat as plain absolute path, not a URL schema
} else if first_separator.is_none() || colon_idx < first_separator.unwrap() {
    Some(colon_idx)
} else {
    None
}
```

**`src/paths/path.rs` — `AnchorPath::is_absolute`**

Extended to recognize Windows drive-letter absolute paths:

```rust
pub fn is_absolute(&self) -> bool {
    let d = self.dir();
    d.starts_with('/')
        || (d.len() >= 3
            && d.as_bytes()[0].is_ascii_alphabetic()
            && d.as_bytes()[1] == b':'
            && d.as_bytes()[2] == b'/')
}
```

**`src/paths/path.rs` — `AnchorPath::canonicalize`**

Strips `X:/` prefix (in addition to the existing `/` strip) when producing
root-relative canonical paths.

**Tests updated** — `test_schema_edge_cases` and `test_windows_absolute_paths` updated to
assert the new correct behaviour: `has_schema()` → false, `filepath()`/`dir()` include
the `C:` prefix, `is_absolute()` → true. New assertions added for `normalize()` and
`join()` with drive-letter bases, plus the `starts_with` guard that mirrors
`regularize_unchecked`.

## Downstream effects (all correct automatically)
- `filepath()` / `dir()` — `sch_sep = None` → `start_idx = 0` → includes `C:` prefix ✓
- `normalize()` — operates on `filepath()` which now includes `C:`, prefix reconstruction
  from `sch_sep` is skipped (None) so the drive letter is preserved in the normalized output ✓
- `has_schema()` — returns `false` for drive letters ✓
- `canonicalize()` — no longer returns `""` for drive-letter paths ✓
- `join()` — `is_absolute()` fix ensures drive-letter paths are treated as absolute ✓
- `strip_prefix()` — `filepath()` now includes `C:`, so prefix stripping works correctly ✓
- `regularize_unchecked` `starts_with` guard — `"C:/Users/.../repo/file.md".starts_with("C:/Users/.../repo")` now passes ✓

## Test results
- All 239 lib unit tests pass
- All 6 `--features service` codec integration tests pass (including `test_belief_set_builder_bid_generation_and_caching` and `test_belief_set_builder_with_db_cache`)
- All doctests pass

## Previous fix context (now superseded)
The `os_path_to_string` double-slash fix (`7ea4dfc`) was a necessary prerequisite:
it ensured `PathBuf("C:\foo")` → `"C:/foo"` (single slash, not `"C://foo"`).
The drive-letter-aware `AnchorPath` fix is the completion of that work.