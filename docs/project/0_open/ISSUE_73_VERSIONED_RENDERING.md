# Issue 73: Version Selector UI — SPA Viewer Dropdown for Multi-Version Deployments

**Priority**: MEDIUM
**Estimated Effort**: 1-2 days (RELATIVE COMPARISON ONLY)
**Dependencies**: None (CI orchestration is the consuming project's responsibility)

## Summary

Add a version-selector dropdown to the SPA viewer that reads a `versions.json` manifest and lets readers switch between documentation versions. noet-core provides the viewer JS and CSS; the consuming project's CI is responsible for building each version, arranging the output directory layout, and producing `versions.json`.

## Goals

1. A `version-selector.js` viewer module that fetches `versions.json`, renders a `<select>` dropdown, and navigates to the equivalent page in the selected version.
2. The selector auto-hides when `versions.json` is not found or the fetch fails (single-version mode, backward-compatible).
3. The `v/` directory prefix is part of the contract between CI and the viewer.
4. A documented `versions.json` schema that consuming projects implement.
5. No changes to noet-core CLI, `GitCache`, or `generate_spa_shell`.

## Architecture

### Responsibility Split

**noet-core provides:**
- `assets/viewer/version-selector.js` — fetches manifest, renders dropdown, handles navigation
- CSS for the dropdown (in `assets/noet-layout.css`)
- The responsive SPA template includes the JS unconditionally; it auto-hides when no manifest is found

**Consuming project (CI) provides:**
- Multi-version directory layout (`output/v/<tag>/`)
- Per-version `noet parse` invocations with appropriate `--base-url`
- `versions.json` manifest at the site root
- Optional `index.html` redirect at the site root

### Version Directory Layout (CI responsibility)

```
output/
  index.html              ← redirect to latest version (CI writes this)
  versions.json           ← manifest of all rendered versions (CI writes this)
  v/
    latest/               ← HEAD build
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

### `versions.json` Schema

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
    },
    {
      "label": "v1.0.0",
      "path": "v/v1.0.0/"
    }
  ]
}
```

- `label` (string, required): Display text in the dropdown.
- `path` (string, required): Relative path from the site root to this version's directory (must end with `/`).

The schema is intentionally minimal. Consuming projects may add additional fields (e.g., `commit`, `date`, `latest` flag) — the JS ignores unknown keys.

### Manifest Resolution

The JS resolves `versions.json` by walking up from the current page's pathname:

1. Match the current URL against `/v/<version>/` using the regex `/\/v\/[^/]+\//`.
2. If matched, the manifest URL is the path prefix before `v/` plus `versions.json` (e.g., `/repo/versions.json`).
3. If not matched (single-version deployment), the selector stays hidden.

### Version Switching

When the user selects a version:

1. Extract the current hash fragment (`window.location.hash`).
2. Replace the `/v/<current>/` segment in the pathname with the selected version's `path`.
3. Navigate to the new URL, preserving the hash fragment.

If the target page doesn't exist in the selected version, the SPA's existing "page not found" handling covers it gracefully.

### SPA Template Integration

The responsive template (`assets/template-responsive.html`) loads `version-selector.js` as a deferred ES module alongside the other viewer scripts. The `<select>` element is prepended to `.noet-nav-header` (above the search box). No conditional logic is needed in the template — the JS handles show/hide.

## Implementation Steps

### 1. Version selector JS — `assets/viewer/version-selector.js` (0.5-1 day)

- [x] Create `version-selector.js` following existing viewer module patterns
- [x] On init, resolve `versions.json` URL from current pathname via `/v/<version>/` regex
- [x] If URL cannot be resolved (no `/v/` in path), return silently (single-version mode)
- [x] Fetch manifest; on non-200 or parse error, return silently
- [x] Create `<select>` element with `class="noet-version-selector"` and `aria-label="Documentation version"`
- [x] Populate options from `versions.json` entries
- [x] Identify current version by matching `window.location.pathname` against each entry's `path`
- [x] On change, navigate with hash-preserving logic
- [x] Prepend selector to `.noet-nav-header`
- [x] Wire into `viewer.js` — import + `initVersionSelector()` call

### 2. CSS — `assets/noet-layout.css` (0.25 day)

- [x] Style `.noet-version-selector` to match existing header controls
- [x] Theme-aware via `var(--noet-*)` custom properties
- [x] `:focus-visible` outline for accessibility

### 3. Template integration — `assets/template-responsive.html`

- [x] Not needed — `version-selector.js` is an ES module imported by `viewer.js`, which is already loaded by the template

### 4. Manifest assembly script — `scripts/assemble-versions.sh` (0.25 day)

- [x] Create `scripts/assemble-versions.sh` — scans output directory, writes `versions.json` and root `index.html` redirect
- [x] Accepts `label:dirname` pairs as arguments, validates each directory exists
- [x] Skips missing versions with warning, requires `jq`

### 5. Documentation (0.25 day)

- [x] Document `versions.json` schema in `docs/design/network_authoring.md` §11
- [x] Document the expected directory layout for multi-version deployments
- [x] Document the `v/` prefix contract
- [x] Document `assemble-versions.sh` usage
- [x] Provide example CI workflow snippet

## Testing Requirements

### Manual Tests
- Version selector dropdown appears when `versions.json` exists and current URL contains `/v/`
- Version selector is hidden when URL does not contain `/v/`
- Version selector is hidden when `versions.json` fetch fails (404, network error)
- Switching versions preserves the current page's hash fragment
- Dropdown correctly identifies and pre-selects the current version
- Selector renders correctly in both light and dark themes
- Single-version builds (no `versions.json`) produce identical behavior to pre-issue — no selector shown

### Browser Tests
- `version-selector.js` can be loaded by the WASM interface test harness
- No console errors when `versions.json` is absent

## Success Criteria

- [ ] Version selector dropdown appears in the SPA viewer when `versions.json` is served at the expected location
- [ ] Switching versions preserves the current page (hash fragment)
- [ ] Single-version builds produce identical output to pre-issue behavior — no selector shown, no console errors
- [ ] CSS matches existing header controls styling in both themes

## Risks

- **Cross-version page existence**: a page may exist in v2.0 but not in v1.0. The version selector would navigate to a 404. → **Mitigation**: the SPA's existing "page not found" handling covers this gracefully. Optionally, the selector can check page existence before navigating (future enhancement).

- **Non-standard base URL structures**: some deployments may not use the `/v/` prefix. → **Mitigation**: the `/v/` prefix is part of the documented contract. The JS fails gracefully (hides selector) if the pattern doesn't match.

## Out of Scope

The following are explicitly **not** part of this issue. They are the consuming project's responsibility or future enhancements:

- **`--version-tag` CLI flag**: not needed; CI controls version identity.
- **`versions-manifest` subcommand**: not needed; CI assembles `versions.json`.
- **`version.json` per-build sidecar**: not needed; CI knows what it built.
- **`data-version` attribute on `<html>`**: not needed; the JS infers the current version from the URL.
- **Git tag detection / `GitCache` changes**: not needed; CI sets `--base-url` appropriately.
- **Role-scoped entry points**: will compose on top of version selection (see `docs/project/UX_AUDIT.md`).
- **Version diff UI**: structural diff between two versions is a separate feature.
- **Versioned MCP access**: already possible via `noet mcp --output-dir output/v/v1.0`.

## Design Note: CI-Orchestrated Versioning

Traditional documentation versioning is page-level. noet versions the **graph**: each version is a complete BeliefBase at a specific git state. The version selector switches between graph snapshots.

The design deliberately places build orchestration in CI rather than in noet-core:

1. **CI controls git checkouts** — it knows which tags to build and can parallelize.
2. **CI controls directory layout** — it writes each version to `output/v/<tag>/` with the correct `--base-url`.
3. **CI assembles the manifest** — a simple shell/jq script produces `versions.json`.
4. **noet-core stays simple** — it renders one version at a time; the viewer reads a sidecar file.

This keeps noet application-neutral (per AGENTS.md) and avoids coupling the tool to any specific CI system.

## References

- Issue 26: Git-Aware Networks (COMPLETE) — provides git metadata per node
- Issue 59: Git Metadata Export and Asset Dirs (COMPLETE) — provides `source_url` construction
- `docs/project/UX_AUDIT.md` §3 — role-scoped entry points compose on top of versioning
- `assets/viewer/network-selector.js` — similar pattern for the existing network dropdown
- `assets/viewer/state.js` — shared state module that new JS imports from
- `assets/noet-layout.css` — layout styles for new CSS
- `assets/template-responsive.html` — SPA shell template for script inclusion
