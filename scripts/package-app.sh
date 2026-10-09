#!/usr/bin/env bash
# Compatibility entry point: package the unified Tauri app and macOS installer.

set -euo pipefail

ROOT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"

exec "$ROOT_DIR/scripts/build-edition.sh" --bundles dmg,app "$@"
