#!/usr/bin/env bash
# Build UsageCheck installer bundles for Free or Pro edition.
#
# Usage:
#   ./scripts/build-edition.sh free [tauri bundle args...]
#   ./scripts/build-edition.sh pro  [tauri bundle args...]
#
# Examples:
#   ./scripts/build-edition.sh free --bundles dmg,app
#   ./scripts/build-edition.sh pro  --bundles nsis,msi

set -euo pipefail

ROOT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
EDITION="${1:-}"
shift || true

if [[ "$EDITION" != "free" && "$EDITION" != "pro" ]]; then
  echo "usage: $0 <free|pro> [cargo tauri build args...]" >&2
  exit 1
fi

mkdir -p "$ROOT_DIR/ui/dist"
printf '%s\n' '<!doctype html><html><body></body></html>' > "$ROOT_DIR/ui/dist/index.html"

cd "$ROOT_DIR/src-tauri"

if [[ "$EDITION" == "free" ]]; then
  exec cargo tauri build \
    --no-default-features \
    --features custom-protocol,edition-free \
    "$@"
else
  exec cargo tauri build \
    --no-default-features \
    --features custom-protocol,edition-pro \
    --config tauri.pro.conf.json \
    "$@"
fi
