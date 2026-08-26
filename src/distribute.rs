//! Distribution packaging for offline documentation delivery
//!
//! Assembles a self-contained directory that stakeholders can run on any
//! platform to browse rendered HTML documentation locally — no install required.
//!
//! The output directory contains:
//! - `site/` — a recursive copy of the rendered HTML site
//! - `miniserve.exe` — embedded static file server (Windows)
//! - `serve.bat` — Windows launcher
//! - `serve.sh` — Linux launcher (uses Python's built-in HTTP server)
//! - `serve.command` — macOS launcher (double-click to start; same script as `serve.sh`)
//! - `README.md` — stakeholder-facing instructions

use std::fs;
#[cfg(unix)]
use std::os::unix::fs::PermissionsExt;
use std::path::Path;

use crate::error::BuildonomyError;

/// Embedded miniserve binary for Windows distribution.
///
/// This mirrors the WASM embedding pattern in `src/codec/assets.rs` — the binary
/// is compiled into the `noet` executable at build time so the distribute command
/// is fully self-contained with no runtime downloads.
#[cfg(feature = "distribute")]
const MINISERVE_WINDOWS: &[u8] = include_bytes!("../vendor/miniserve-x86_64-pc-windows-msvc.exe");

/// Assemble a distributable documentation package at `target`.
///
/// `site_path` must be an existing directory containing an `index.html`.
/// `port` is baked into the generated launcher scripts.
pub fn distribute(site_path: &Path, target: &Path, port: u16) -> Result<(), BuildonomyError> {
    // -- Validate source -------------------------------------------------
    if !site_path.is_dir() {
        return Err(BuildonomyError::NotFound(format!(
            "Site path does not exist or is not a directory: {}",
            site_path.display()
        )));
    }
    if !site_path.join("index.html").is_file() {
        return Err(BuildonomyError::NotFound(format!(
            "Site path does not contain an index.html: {}",
            site_path.display()
        )));
    }

    // -- Create target directory -----------------------------------------
    fs::create_dir_all(target).map_err(|e| {
        BuildonomyError::Io(format!(
            "Failed to create target directory {}: {e}",
            target.display()
        ))
    })?;

    // -- Copy site -------------------------------------------------------
    // Canonicalize the target so we can skip it if it lives inside the source
    // tree, preventing infinite recursion in copy_dir_recursive.
    let skip_dir = target.canonicalize().ok();
    let site_dest = target.join("site");
    copy_dir_recursive(site_path, &site_dest, skip_dir.as_deref())?;

    // -- Write miniserve binary ------------------------------------------
    #[cfg(feature = "distribute")]
    {
        let miniserve_path = target.join("miniserve.exe");
        fs::write(&miniserve_path, MINISERVE_WINDOWS).map_err(|e| {
            BuildonomyError::Io(format!(
                "Failed to write miniserve.exe to {}: {e}",
                miniserve_path.display()
            ))
        })?;
    }

    // -- Generate serve.bat ----------------------------------------------
    let serve_bat_path = target.join("serve.bat");
    let serve_bat_content = generate_serve_bat(port);
    fs::write(&serve_bat_path, serve_bat_content).map_err(|e| {
        BuildonomyError::Io(format!(
            "Failed to write serve.bat to {}: {e}",
            serve_bat_path.display()
        ))
    })?;

    // -- Generate serve.sh and serve.command ------------------------------
    let serve_sh_content = generate_serve_sh(port);
    for name in ["serve.sh", "serve.command"] {
        let path = target.join(name);
        fs::write(&path, &serve_sh_content).map_err(|e| {
            BuildonomyError::Io(format!("Failed to write {name} to {}: {e}", path.display()))
        })?;

        #[cfg(unix)]
        {
            fs::set_permissions(&path, fs::Permissions::from_mode(0o755)).map_err(|e| {
                BuildonomyError::Io(format!("Failed to set permissions on {name}: {e}"))
            })?;
        }
    }

    // -- Generate README.md ----------------------------------------------
    let readme_path = target.join("README.md");
    let readme_content = generate_readme(port);
    fs::write(&readme_path, readme_content).map_err(|e| {
        BuildonomyError::Io(format!(
            "Failed to write README.md to {}: {e}",
            readme_path.display()
        ))
    })?;

    Ok(())
}

/// Recursively copy a directory tree from `src` to `dst`.
///
/// Creates `dst` and all intermediate parent directories. Files are copied
/// with `std::fs::copy`; symlinks are followed (not preserved).
///
/// If `skip_canonical` is `Some`, any source subdirectory whose canonical path
/// matches it is silently skipped. This prevents infinite recursion when the
/// target directory lives inside the source tree.
fn copy_dir_recursive(
    src: &Path,
    dst: &Path,
    skip_canonical: Option<&Path>,
) -> Result<(), BuildonomyError> {
    fs::create_dir_all(dst).map_err(|e| {
        BuildonomyError::Io(format!("Failed to create directory {}: {e}", dst.display()))
    })?;

    let entries = fs::read_dir(src).map_err(|e| {
        BuildonomyError::Io(format!("Failed to read directory {}: {e}", src.display()))
    })?;

    for entry in entries {
        let entry = entry.map_err(|e| {
            BuildonomyError::Io(format!(
                "Failed to read directory entry in {}: {e}",
                src.display()
            ))
        })?;
        let src_path = entry.path();
        let dst_path = dst.join(entry.file_name());

        if src_path.is_dir() {
            if let Some(skip) = skip_canonical {
                if src_path.canonicalize().ok().as_deref() == Some(skip) {
                    continue;
                }
            }
            copy_dir_recursive(&src_path, &dst_path, skip_canonical)?;
        } else {
            fs::copy(&src_path, &dst_path).map_err(|e| {
                BuildonomyError::Io(format!(
                    "Failed to copy {} to {}: {e}",
                    src_path.display(),
                    dst_path.display()
                ))
            })?;
        }
    }

    Ok(())
}

fn generate_serve_bat(port: u16) -> String {
    format!(
        "@echo off\r\n\
         cd /d \"%~dp0\"\r\n\
         set PORT={port}\r\n\
         set /a MAX_PORT=PORT+20\r\n\
         :find_port\r\n\
         netstat -an | findstr \":%PORT% \" | findstr LISTENING >nul 2>&1\r\n\
         if errorlevel 1 goto port_ok\r\n\
         echo Port %PORT% is in use, trying next...\r\n\
         set /a PORT=PORT+1\r\n\
         if %PORT% LEQ %MAX_PORT% goto find_port\r\n\
         echo ERROR: Could not find an available port ({port}-%MAX_PORT%).\r\n\
         pause\r\n\
         exit /b 1\r\n\
         :port_ok\r\n\
         echo Starting documentation server...\r\n\
         echo.\r\n\
         echo Open your browser to:  http://localhost:%PORT%\r\n\
         echo Press Ctrl+C in this window to stop the server.\r\n\
         echo.\r\n\
         start http://localhost:%PORT%\r\n\
         miniserve.exe --index index.html --port %PORT% site\r\n"
    )
}

fn generate_serve_sh(port: u16) -> String {
    format!(
        r#"#!/bin/sh
SCRIPT_DIR="$(cd "$(dirname "$0")" && pwd)"
PORT={port}
MAX_PORT=$((PORT + 20))

# Find an available port
while [ "$PORT" -le "$MAX_PORT" ]; do
    if ! (echo >/dev/tcp/localhost/$PORT) 2>/dev/null; then
        break
    fi
    echo "Port $PORT is in use, trying $((PORT + 1))..."
    PORT=$((PORT + 1))
done

if [ "$PORT" -gt "$MAX_PORT" ]; then
    echo "ERROR: Could not find an available port ({port}-$MAX_PORT)." >&2
    exit 1
fi

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
cd "$SCRIPT_DIR/site"
python3 -m http.server "$PORT"
"#
    )
}

fn generate_readme(port: u16) -> String {
    format!(
        "\
# Documentation Viewer

This directory contains a self-contained documentation package that you can
browse locally in your web browser — no installation required.

## Quick Start

### Windows

Double-click **`serve.bat`**, or run it from a command prompt:

```
serve.bat
```

This starts a local web server using the bundled `miniserve.exe` and opens
your browser to <http://localhost:{port}>.

### macOS

Double-click **`serve.command`**. A Terminal window opens, starts the server,
and opens your browser to <http://localhost:{port}>.

### Linux

Open a terminal in this directory and run:

```
chmod +x serve.sh   # only needed once, after unzipping
./serve.sh
```

This starts a local web server and opens your browser to <http://localhost:{port}>.

> **Prerequisite (macOS / Linux):** Python 3 must be installed and available as
> `python3` on your PATH. Most macOS and Linux systems include it by default.

## Stopping the Server

Press **Ctrl+C** in the terminal / command prompt window where the server is
running.

## Contents

| File / Directory | Purpose |
|------------------|---------|
| `site/`          | The rendered documentation (HTML, CSS, JS, assets). |
| `serve.bat`      | Windows launcher — starts `miniserve.exe` on port {port}. |
| `serve.sh`       | Linux launcher — starts Python's HTTP server on port {port}. |
| `serve.command`  | macOS launcher — double-click to start (same as `serve.sh`). |
| `miniserve.exe`  | Static file server for Windows (no install needed). |
| `README.md`      | This file. |

## Licenses

The bundled `miniserve.exe` is
[miniserve](https://github.com/svenstaro/miniserve) by Sven-Hendrik Haase,
distributed under the **MIT License**. See the miniserve repository for full
license text.
"
    )
}
