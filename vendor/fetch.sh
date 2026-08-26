#!/bin/sh
# Fetch vendored third-party binaries.
#
# Usage:
#   ./vendor/fetch.sh           # download if not already present
#   ./vendor/fetch.sh --force   # re-download even if present
#
# This script is idempotent: it skips downloads when the target file
# already exists unless --force is passed.

set -e

VENDOR_DIR="$(cd "$(dirname "$0")" && pwd)"

MINISERVE_VERSION="0.35.0"
MINISERVE_BINARY="miniserve-x86_64-pc-windows-msvc.exe"
MINISERVE_URL="https://github.com/svenstaro/miniserve/releases/download/v${MINISERVE_VERSION}/miniserve-${MINISERVE_VERSION}-x86_64-pc-windows-msvc.exe"
MINISERVE_DEST="${VENDOR_DIR}/${MINISERVE_BINARY}"

FORCE=0
for arg in "$@"; do
    case "$arg" in
        --force) FORCE=1 ;;
        *)
            echo "Usage: $0 [--force]" >&2
            exit 1
            ;;
    esac
done

if [ -f "${MINISERVE_DEST}" ] && [ "${FORCE}" -eq 0 ]; then
    echo "Already present: ${MINISERVE_DEST} (use --force to re-download)"
    exit 0
fi

echo "Downloading miniserve v${MINISERVE_VERSION}..."
curl -L --fail --retry 3 -o "${MINISERVE_DEST}" "${MINISERVE_URL}"

if [ ! -f "${MINISERVE_DEST}" ]; then
    echo "ERROR: download failed — ${MINISERVE_DEST} not found" >&2
    exit 1
fi

echo "Downloaded: ${MINISERVE_DEST}"
