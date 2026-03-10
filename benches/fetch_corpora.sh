#!/usr/bin/env bash
# fetch_corpora.sh — Download benchmark corpora for macro-benchmarks
#
# Usage: bash benches/fetch_corpora.sh [--force]
#
# Clones mdn/content at a pinned SHA into .bench_corpora/ using a sparse
# checkout so only *.md files are materialised (skipping images and other
# binary assets). Idempotent: skips repos already at the correct SHA.
# Pass --force to re-clone regardless.
#
# The MDN content repo uses the layout:
#
#   files/en-us/<topic>/index.md
#   files/en-us/<topic>/<subtopic>/index.md
#   ...
#
# Every subdirectory already contains an index.md, but the root
# files/en-us/ directory does not. A minimal index.md is generated there
# after the clone so NetworkCodec treats it as the network root.
#
# Pinned SHA: update this when you want to advance the corpus baseline.

set -euo pipefail

CORPORA_DIR="${BENCH_CORPORA_DIR:-.bench_corpora}"
FORCE=0

for arg in "$@"; do
  case "$arg" in
    --force) FORCE=1 ;;
    *) echo "Unknown argument: $arg" >&2; exit 1 ;;
  esac
done

# ---------------------------------------------------------------------------
# Corpus definition
# ---------------------------------------------------------------------------
# mdn/content @ 2025-06: ~14 000 index.md files, ~55 MB of markdown.
MDN_NAME="mdn-content"
MDN_URL="https://github.com/mdn/content.git"
MDN_SHA="6c53947ceb7d71352b382f9d6564d021d7fe376e"

# ---------------------------------------------------------------------------

clone_or_update() {
  local name="$1"
  local url="$2"
  local sha="$3"
  local dest="$CORPORA_DIR/$name"

  if [[ -d "$dest" ]]; then
    local current_sha
    current_sha=$(git -C "$dest" rev-parse HEAD 2>/dev/null || echo "unknown")

    if [[ "$current_sha" == "$sha" && "$FORCE" -eq 0 ]]; then
      echo "  ✓ $name already at ${sha:0:12} — skipping"
      return
    fi

    echo "  ↻ $name exists but SHA mismatch (have: ${current_sha:0:12}, want: ${sha:0:12}) — re-cloning"
    rm -rf "$dest"
  fi

  echo "  ↓ Cloning $name from $url @ ${sha:0:12}..."

  # Initialise a bare-minimum clone with no blobs checked out.
  # --filter=blob:none defers blob download until checkout; combined with a
  # sparse checkout of only *.md paths this keeps the materialised footprint
  # to the markdown files only (no PNG/SVG/GIF/MP4 etc.).
  git clone \
    --no-checkout \
    --filter=blob:none \
    --sparse \
    "$url" "$dest"

  # Configure sparse checkout to include only markdown files.
  # We want every index.md under files/en-us/ (the actual content tree).
  git -C "$dest" sparse-checkout set --no-cone 'files/en-us/**/*.md'

  # Checkout the pinned SHA. This materialises only the matched .md blobs.
  git -C "$dest" checkout "$sha" --quiet

  echo "  ✓ $name cloned at ${sha:0:12}"
}

write_root_index() {
  local dest="$1"
  local root_index="$dest/files/en-us/index.md"
  # Only write if missing — preserve any manually placed index.md.
  if [[ ! -f "$root_index" ]]; then
    cat > "$root_index" <<'EOF'
---
id = "mdn-en-us"
title = "MDN Web Docs (en-US)"
---

<!-- network-children -->
EOF
    echo "  ✎ generated root index.md at $root_index"
  else
    echo "  ✓ root index.md already exists — skipping"
  fi
}

echo "Fetching benchmark corpora into $CORPORA_DIR/"
mkdir -p "$CORPORA_DIR"

clone_or_update "$MDN_NAME" "$MDN_URL" "$MDN_SHA"
write_root_index "$CORPORA_DIR/$MDN_NAME"

echo ""
echo "Corpus inventory:"
dest="$CORPORA_DIR/$MDN_NAME"
if [[ -d "$dest" ]]; then
  md_count=$(find "$dest/files/en-us" -name "*.md" 2>/dev/null | wc -l | tr -d ' ')
  total_kb=$(du -sk "$dest/files/en-us" 2>/dev/null | cut -f1)
  echo "  $MDN_NAME: $md_count .md files, ~${total_kb} KB"
else
  echo "  $MDN_NAME: MISSING"
fi
