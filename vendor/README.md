# vendor/

Vendored third-party binaries used by the `distribute` feature.

These binaries are **committed to the repository** and embedded into the
`noet` binary at compile time via `include_bytes!` (see `src/distribute.rs`) —
the same pattern used for UI assets in `assets/` (see `CONTRIBUTING.md` §
"UI Asset Workflow"). This keeps `cargo build` / `cargo install` fully
offline-capable: no build-time network fetch, no `docs.rs` build-sandbox
failure, no extra setup step for contributors or downstream consumers.

## Contents

| Binary | Version | Purpose |
|--------|---------|---------|
| `miniserve-x86_64-pc-windows-msvc.exe` | 0.35.0 | Embedded HTTP server bundled into Windows distributions |

## Why vendor?

The `distribute` feature packages a self-contained archive that includes a
lightweight HTTP server for browsing rendered output. On Windows there is no
system-provided HTTP server, so we bundle
[miniserve](https://github.com/svenstaro/miniserve) — a single-file, zero-config
static file server.

## Updating a vendored binary

```sh
curl -L --fail -o vendor/miniserve-x86_64-pc-windows-msvc.exe \
  https://github.com/svenstaro/miniserve/releases/download/v<VERSION>/miniserve-<VERSION>-x86_64-pc-windows-msvc.exe
```

Bump the version in the table above and commit the new binary alongside the
version bump.
