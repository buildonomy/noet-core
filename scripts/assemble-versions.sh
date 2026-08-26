#!/usr/bin/env sh
# assemble-versions.sh — Generate versions.json manifest and root index.html
#
# Scans an output directory for versioned builds under v/<dirname>/ and produces
# the versions.json manifest consumed by the noet SPA version-selector dropdown.
#
# Usage:
#   assemble-versions.sh <output-dir> <label>:<dirname> [<label>:<dirname> ...]
#
# Arguments:
#   <output-dir>       Root of the multi-version site (e.g., _site/)
#   <label>:<dirname>  One or more version entries. <label> is the display text
#                      in the dropdown; <dirname> is the subdirectory under v/.
#                      Order determines dropdown order (first = default/top).
#
# Examples:
#   # Two versions — "latest" first (becomes the redirect target)
#   assemble-versions.sh _site "Latest (main):latest" "v2.0.0:v2.0.0"
#
#   # Using branch names — dirname can be anything
#   assemble-versions.sh _site "Development:dev" "Release 1.0:v1.0.0"
#
# Behavior:
#   - Entries whose v/<dirname>/index.html does not exist are skipped with a warning.
#   - If fewer than 2 versions have content, versions.json is still written (the
#     viewer auto-hides the selector for <2 entries).
#   - The root index.html is a meta-refresh redirect to the first listed version.
#
# Output:
#   <output-dir>/versions.json  — manifest for the SPA version selector
#   <output-dir>/index.html     — redirect to the first listed version
#
# Requirements:
#   jq (https://jqlang.github.io/jq/) — installed by default on GitHub Actions runners.
#
# References:
#   - Issue 73: Version Selector UI
#   - assets/viewer/version-selector.js (consumer of versions.json)

set -eu

# ---------------------------------------------------------------------------
# Argument parsing
# ---------------------------------------------------------------------------

if [ $# -lt 2 ]; then
  echo "Usage: assemble-versions.sh <output-dir> <label>:<dirname> [...]" >&2
  echo "  Each <dirname> must exist as <output-dir>/v/<dirname>/" >&2
  exit 1
fi

OUTDIR="$1"
shift

if [ ! -d "$OUTDIR" ]; then
  echo "Error: output directory '$OUTDIR' does not exist" >&2
  exit 1
fi

if ! command -v jq >/dev/null 2>&1; then
  echo "Error: jq is required but not installed" >&2
  echo "  Install: https://jqlang.github.io/jq/download/" >&2
  exit 1
fi

# ---------------------------------------------------------------------------
# Build version list
# ---------------------------------------------------------------------------

VERSIONS="[]"
FIRST_PATH=""
FOUND=0
SKIPPED=0

for pair in "$@"; do
  # Split on first colon — label may contain colons, dirname should not.
  label="${pair%%:*}"
  dirname="${pair#*:}"

  if [ -z "$label" ] || [ -z "$dirname" ]; then
    echo "Warning: skipping malformed entry '$pair' (expected label:dirname)" >&2
    SKIPPED=$((SKIPPED + 1))
    continue
  fi

  vdir="$OUTDIR/v/$dirname"
  if [ ! -f "$vdir/index.html" ]; then
    echo "Warning: skipping '$dirname' — no index.html in $vdir" >&2
    SKIPPED=$((SKIPPED + 1))
    continue
  fi

  VERSIONS=$(printf '%s' "$VERSIONS" | jq \
    --arg label "$label" \
    --arg path "v/$dirname/" \
    '. + [{"label": $label, "path": $path}]')

  if [ -z "$FIRST_PATH" ]; then
    FIRST_PATH="v/$dirname/"
  fi

  FOUND=$((FOUND + 1))
done

# ---------------------------------------------------------------------------
# Write versions.json
# ---------------------------------------------------------------------------

printf '%s' "$VERSIONS" | jq '{versions: .}' > "$OUTDIR/versions.json"
echo "✓ Wrote $OUTDIR/versions.json ($FOUND versions, $SKIPPED skipped)"

# ---------------------------------------------------------------------------
# Write root index.html redirect
# ---------------------------------------------------------------------------

if [ -n "$FIRST_PATH" ]; then
  cat > "$OUTDIR/index.html" <<REDIRECT_EOF
<!DOCTYPE html>
<html lang="en">
<head>
  <meta charset="utf-8">
  <meta http-equiv="refresh" content="0; url=$FIRST_PATH">
  <link rel="canonical" href="$FIRST_PATH">
  <title>Redirecting…</title>
</head>
<body>
  <p>Redirecting to <a href="$FIRST_PATH">latest documentation</a>…</p>
</body>
</html>
REDIRECT_EOF
  echo "✓ Wrote $OUTDIR/index.html (redirect → $FIRST_PATH)"
else
  echo "Warning: no valid versions found — root index.html not written" >&2
fi
