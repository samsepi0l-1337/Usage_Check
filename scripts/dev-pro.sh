#!/usr/bin/env bash
# Run a debug UsageCheck build against the local real-token dev server.

set -euo pipefail

ROOT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
DEV_DIR="$ROOT_DIR/.dev"
SERVER_PORT="${USAGECHECK_DEV_LICENSE_PORT:-5179}"
KEY_FILE="${USAGECHECK_DEV_LICENSE_KEY_FILE:-$DEV_DIR/license-signing-key}"
SERVER_LOG="$DEV_DIR/dev-license-server.log"

"$ROOT_DIR/scripts/bootstrap-dev.sh"
mkdir -p "$DEV_DIR"
: > "$SERVER_LOG"

cleanup() {
  if [[ -n "${SERVER_PID:-}" ]] && kill -0 "$SERVER_PID" 2>/dev/null; then
    kill "$SERVER_PID" 2>/dev/null || true
    wait "$SERVER_PID" 2>/dev/null || true
  fi
}
trap cleanup EXIT INT TERM

(
  cd "$ROOT_DIR"
  exec cargo run -p usage-app --example dev_license_server -- \
    --port "$SERVER_PORT" --key-file "$KEY_FILE"
) > "$SERVER_LOG" 2>&1 &
SERVER_PID=$!

for _ in $(seq 1 100); do
  if grep -q '^USAGECHECK_LICENSE_PUBKEY=' "$SERVER_LOG" \
    && grep -q '^USAGECHECK_LICENSE_API=' "$SERVER_LOG"; then
    break
  fi
  if ! kill -0 "$SERVER_PID" 2>/dev/null; then
    echo "dev-pro: development license server did not start; see $SERVER_LOG" >&2
    exit 1
  fi
  sleep 0.1
done

USAGECHECK_LICENSE_PUBKEY="$(
  sed -n '/^USAGECHECK_LICENSE_PUBKEY=/{s/^USAGECHECK_LICENSE_PUBKEY=//;p;q;}' "$SERVER_LOG"
)"
USAGECHECK_LICENSE_API="$(
  sed -n '/^USAGECHECK_LICENSE_API=/{s/^USAGECHECK_LICENSE_API=//;p;q;}' "$SERVER_LOG"
)"
if [[ -z "$USAGECHECK_LICENSE_PUBKEY" || -z "$USAGECHECK_LICENSE_API" ]]; then
  echo "dev-pro: timed out waiting for the development license server; see $SERVER_LOG" >&2
  exit 1
fi

export USAGECHECK_LICENSE_PUBKEY
export USAGECHECK_LICENSE_API
export USAGECHECK_APP_DATA_DIR="${USAGECHECK_APP_DATA_DIR:-$DEV_DIR/app-data}"

echo 'Next: copy any key, open the UsageCheck tray menu, then choose Activate from clipboard.'
echo "Using isolated app data at $USAGECHECK_APP_DATA_DIR"
cd "$ROOT_DIR"
cargo run -p usage-app
