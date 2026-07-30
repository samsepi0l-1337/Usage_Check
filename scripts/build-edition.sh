#!/usr/bin/env bash
# Build a UsageCheck installer bundle.
#
# UsageCheck ships as a single unified binary (Free by default; a Pro
# license key unlocks Cursor/Grok/Higgsfield at runtime — see
# `docs/LICENSE_API.md`). There is no separate Free/Pro build anymore.
#
# Usage:
#   ./scripts/build-edition.sh [tauri bundle args...]
#
# Examples:
#   ./scripts/build-edition.sh --bundles dmg,app
#   ./scripts/build-edition.sh --bundles nsis,msi

set -euo pipefail

ROOT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"

mkdir -p "$ROOT_DIR/ui/dist"
printf '%s\n' '<!doctype html><html><body></body></html>' > "$ROOT_DIR/ui/dist/index.html"

cd "$ROOT_DIR/src-tauri"

exec cargo tauri build \
  --features custom-protocol \
  "$@"
