# vendor/

Vendored third-party binaries used by the `distribute` feature.

These binaries are **not committed to the repository**. They are fetched on
demand by `fetch.sh` and ignored via `.gitignore`.

## Contents

| Binary | Version | Purpose |
|--------|---------|---------|
| `miniserve-x86_64-pc-windows-msvc.exe` | 0.35.0 | Embedded HTTP server bundled into Windows distributions |

## Usage

From the repository root:

```sh
# Download all vendored binaries (skips if already present)
./vendor/fetch.sh

# Force re-download
./vendor/fetch.sh --force
```

## Why vendor?

The `distribute` feature packages a self-contained archive that includes a
lightweight HTTP server for browsing rendered output. On Windows there is no
system-provided HTTP server, so we bundle
[miniserve](https://github.com/svenstaro/miniserve) — a single-file, zero-config
static file server.

The binary is fetched from the official GitHub release rather than checked in,
keeping the repository small while ensuring reproducible builds.
