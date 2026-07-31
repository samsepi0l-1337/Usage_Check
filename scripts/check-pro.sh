#!/usr/bin/env bash
# Check a running local UsageCheck API for Pro-gating regressions.

set -euo pipefail

ROOT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
"$ROOT_DIR/scripts/bootstrap-dev.sh"

API_PORT="${USAGECHECK_API_PORT:-5178}"
API_BASE="http://127.0.0.1:$API_PORT"
TMP_DIR="$(mktemp -d "${TMPDIR:-/tmp}/usagecheck-pro-check.XXXXXX")"
trap 'rm -rf "$TMP_DIR"' EXIT

CURL_ARGS=(-fsS)
if [[ -n "${USAGECHECK_API_TOKEN:-}" ]]; then
  CURL_ARGS+=(-H "Authorization: Bearer $USAGECHECK_API_TOKEN")
fi

fetch() {
  local path="$1"
  local output="$2"
  if ! curl "${CURL_ARGS[@]}" "$API_BASE$path" -o "$output"; then
    echo "check-pro: API is unreachable or rejected $path at $API_BASE" >&2
    exit 1
  fi
}

fetch /v1/license "$TMP_DIR/license.json"
fetch /v1/accounts "$TMP_DIR/accounts.json"
fetch /v1/usage "$TMP_DIR/usage.json"

python3 - "$TMP_DIR/license.json" "$TMP_DIR/accounts.json" "$TMP_DIR/usage.json" <<'PY'
import json
import sys

license_path, accounts_path, usage_path = sys.argv[1:]
try:
    with open(license_path, encoding="utf-8") as handle:
        license_state = json.load(handle)
    with open(accounts_path, encoding="utf-8") as handle:
        accounts_state = json.load(handle)
    with open(usage_path, encoding="utf-8") as handle:
        usage_state = json.load(handle)
except (OSError, json.JSONDecodeError) as error:
    print(f"check-pro: API returned invalid JSON: {error}", file=sys.stderr)
    sys.exit(1)

status = license_state.get("status")
forced = license_state.get("forced")
if status not in {"free", "pro", "expired", "grace_period_ended"} or not isinstance(forced, bool):
    print("check-pro: /v1/license has an unexpected response shape", file=sys.stderr)
    sys.exit(1)
if not isinstance(accounts_state.get("accounts"), list) or not isinstance(usage_state.get("accounts"), list):
    print("check-pro: /v1/accounts or /v1/usage has an unexpected response shape", file=sys.stderr)
    sys.exit(1)

print("provider\taccount\tstatus")
pro_required = []
paid_count = 0
for account in usage_state["accounts"]:
    provider = account.get("provider", "unknown")
    name = account.get("display_name") or account.get("id", "unknown")
    account_status = account.get("status", "unknown")
    print(f"{provider}\t{name}\t{account_status}")
    if provider in {"cursor", "grok", "higgsfield"}:
        paid_count += 1
        if account_status == "pro_required":
            pro_required.append(f"{provider}:{name}")

print(f"check-pro: paid accounts examined: {paid_count}")

qualifier = " (dev override)" if forced else ""
if paid_count == 0:
    print(
        f"check-pro: license is {status}{qualifier}; no paid accounts are configured, "
        "so Pro gating was not verified."
    )
    sys.exit(0)

if status == "pro" and pro_required:
    print(
        "check-pro: Pro is active but paid providers remain gated: " + ", ".join(pro_required),
        file=sys.stderr,
    )
    sys.exit(2)

print(f"check-pro: license is {status}{qualifier}; no active Pro-gating regression found.")
PY
