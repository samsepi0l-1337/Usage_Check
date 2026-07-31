#!/usr/bin/env bash
# The tray shell is native; this legacy HTML only satisfies Tauri's
# frontendDist requirement for fresh checkouts and is never rendered.

set -euo pipefail

ROOT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
DIST_DIR="$ROOT_DIR/ui/dist"

if [[ -d "$DIST_DIR" ]]; then
  exit 0
fi

mkdir -p "$DIST_DIR"
printf '%s\n' '<!doctype html><html><body></body></html>' > "$DIST_DIR/index.html"
