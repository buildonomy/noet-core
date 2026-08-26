# Issue 84: `noet distribute` Command — Portable Site Packaging

**Priority**: MEDIUM
**Estimated Effort**: 3 days
**Dependencies**: None (uses existing `extract_assets` + parse output pipeline)

## Summary

Users need to ship rendered noet sites to stakeholders who lack developer
tooling. The `_site/` SPA requires an HTTP server (`fetch()` for msgpack
shards), so double-clicking `index.html` doesn't work. `noet distribute`
packages the rendered output alongside a platform-appropriate static file
server and launcher script into a self-contained directory (or zip) that
non-technical users can unzip-and-run.

## Goals

- `noet distribute <site_path>` produces a self-contained distribution
  directory containing a copy of the site, server binaries, launcher scripts,
  and a README
- Support Windows (miniserve .exe + `serve.bat`) and Unix (Python
  `http.server` + `serve.sh`) targets
- Generated README gives stakeholders 4-step instructions (unzip → run →
  browse → stop)
- The command works without network access — server binaries are vendored into
  the noet binary at compile time via `include_bytes!`

## Architecture

### Subcommand

```
noet distribute <site_path> [--target <dest_path>] [--port PORT]
```

- `<site_path>` (positional, required): path to the rendered site (the
  `--html-output` directory from `noet parse`)
- `--target` / `-t`: destination directory (default: `<site_path>_dist`)
- `--port`: port number to embed in launcher scripts (default: 8080)

The site is *copied into* the distribution directory (as a `site/`
subdirectory), keeping the parse output untouched.

### Server Strategy

**Windows**: Vendor a miniserve `.exe` (~3 MB) embedded via `include_bytes!`
behind a cargo feature flag (`feature = "distribute"`). Windows lacks a
default HTTP server, so we ship one.

**Unix (Linux/macOS)**: Use `python3 -m http.server`, which ships with
essentially every modern Unix. No vendored binary needed. The `serve.sh`
script invokes Python directly.

```
vendor/
└── miniserve-x86_64-pc-windows-msvc.exe   # Windows only
```

Use `include_bytes!` gated on `#[cfg(feature = "distribute")]`, mirroring the
existing WASM embedding pattern in `assets.rs`.

### Output Structure

```
<target>/                 # default: <site_path>_dist
├── site/                 # Copied from <site_path>
│   ├── index.html
│   ├── beliefbase/
│   ├── assets/
│   └── pages/
├── serve.bat             # Windows launcher (uses miniserve.exe)
├── serve.sh              # Linux launcher (uses python3 http.server)
├── serve.command         # macOS launcher (double-click to start; same as serve.sh)
├── miniserve.exe         # Windows server (from vendored bytes)
└── README.md             # Stakeholder instructions
```

### Generated Artifacts

**`serve.bat`**:
```bat
@echo off
echo Starting documentation server...
echo.
echo Open your browser to:  http://localhost:{port}
echo Press Ctrl+C in this window to stop the server.
echo.
start http://localhost:{port}
miniserve.exe --index index.html site
```

**`serve.sh`**:
```sh
#!/bin/sh
PORT={port}
echo "Starting documentation server..."
echo ""
echo "Open your browser to:  http://localhost:$PORT"
echo "Press Ctrl+C to stop the server."
echo ""
if command -v xdg-open >/dev/null 2>&1; then
    xdg-open "http://localhost:$PORT" &
elif command -v open >/dev/null 2>&1; then
    open "http://localhost:$PORT" &
fi
cd site
python3 -m http.server "$PORT"
```

**`README.md`**: Platform-specific instructions generated from a template.

## Implementation Steps

1. **Vendor miniserve binary** (0.5 day)
   - [ ] Create `vendor/` directory with download script (`vendor/fetch.sh`)
   - [ ] Add `include_bytes!` for Windows miniserve, gated on `feature = "distribute"`
   - [ ] Add `.gitignore` entry — binary tracked via fetch script, not git
   - [ ] Document the fetch step in `CONTRIBUTING.md`

2. **Add `distribute` subcommand** (1 day)
   - [ ] Add `Distribute` variant to `Commands` enum, gated on `feature = "distribute"`
   - [ ] Implement directory creation, site copying, miniserve.exe extraction
   - [ ] Generate `serve.bat` and `serve.sh` from templates (with port substitution)
   - [ ] Generate README from template

3. **Testing** (0.5 day)
   - [ ] Integration test: distribute produces expected file tree
   - [ ] Launcher scripts are syntactically valid
   - [ ] README contains correct port and instructions
   - [ ] Feature-gated: `cargo test` without `distribute` feature still passes
   - [ ] Vendored binary extraction produces executable files (Unix permissions)

## Testing Requirements

- `noet distribute` with a test fixture site produces the correct directory layout
- Generated scripts reference the correct port
- Without the `distribute` feature, the subcommand is absent (no compile error)
- Unix: `serve.sh` has executable permission bits

## Success Criteria

- [ ] `noet distribute _site` produces a working distribution
- [ ] Stakeholder can unzip and double-click `serve.bat` (Windows) or
      `./serve.sh` (Linux) to browse the site
- [ ] Binary size increase is behind a feature gate, not imposed on all users
- [ ] README in the distribution is clear enough for non-technical users

## Risks

- **Miniserve binary size (~3 MB)**: Bloats the noet binary when `distribute`
  feature is enabled → **Mitigation**: Feature-gated; only users who need
  distribution pay the cost. Only Windows binary is vendored.
- **Miniserve version pinning**: Need to track upstream releases manually →
  **Mitigation**: `vendor/fetch.sh` documents the exact version; CI can
  verify the hash.
- **Licensing**: miniserve is MIT-licensed, compatible with embedding →
  **Mitigation**: Include license notice in generated README.
- **Python availability on Unix**: `python3` could theoretically be missing →
  **Mitigation**: `serve.sh` checks for `python3` and prints a clear error
  message if absent.

## Open Questions

- Zip support deferred to a follow-up — users can zip the output directory
  themselves.
