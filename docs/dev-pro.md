# Local Pro verification

This guide is for local development only. It provides two ways to exercise
the runtime Pro gate without changing the production public key or contacting
the production license endpoint.

**Neither path can unlock a release binary.** The public-key override,
isolated app-data override, and force-Pro flag are all debug-build-only. A
release build still rejects the placeholder embedded public key and therefore
fails closed to Free.

## Step zero: bootstrap a fresh checkout

Run this before a local build:

```sh
./scripts/bootstrap-dev.sh
```

It creates a tiny ignored `ui/dist/index.html` only when `ui/dist` is absent.
UsageCheck's tray UI is native and never renders it; it merely prevents
Tauri's fresh-checkout macro failure:

```text
error: proc macro panicked ... frontendDist ... "../ui/dist" but this path doesn't exist
```

The script is idempotent and does nothing when `ui/dist` already exists.

## Real activation path

Use this path to exercise the real activation request, Ed25519 signature
verification, device binding, persisted license record, and local API state.

```sh
./scripts/dev-pro.sh
```

The script starts `dev_license_server` in the background, creates or reuses a
stable signing key at `.dev/license-signing-key` (mode `0600` on Unix, with
its parent directory restricted to `0700`), and exports all of the following
to the debug app process:

- `USAGECHECK_LICENSE_PUBKEY`: the development server's public key.
- `USAGECHECK_LICENSE_API`: its localhost verify endpoint.
- `USAGECHECK_APP_DATA_DIR`: `.dev/app-data` by default, so the run cannot
  touch normal UsageCheck data. Set this variable before invoking the script
  to choose another isolated directory.

Copy any ordinary value, then choose **Activate from clipboard** in the tray.
The server returns a genuine, freshly signed Pro token for the app's device.
The server prints only its public key and endpoint; it never prints submitted
license keys, device IDs, or tokens. Pressing Ctrl-C exits the app and stops
the background server.

For a standalone server, run:

```sh
cargo run -p usage-app --example dev_license_server -- --port 5179
```

It accepts `--key-file PATH` and `--port PORT`. The example is a Cargo
`examples/` target: it uses `usage-app`'s existing package dependencies but
does not import the application binary crate.

### Failure injection

The submitted key selects development-only server behavior:

| Key | Response | Use |
| --- | --- | --- |
| `invalid` | `400 invalid_key` | Invalid-key display/error path |
| `revoked` | `403 revoked` | Revoked-key error path |
| `device_limit` | `403 device_limit` | Device-cap error path |
| `expired` | Signed `200` token with a past expiry | Client-side expiry logic |
| Any other value | Signed `200` perpetual Pro token | Ordinary local activation |

## Quick UI-only path

For a quick look at Pro-gated menu paths without an activation server:

```sh
./scripts/bootstrap-dev.sh
USAGECHECK_FORCE_PRO=1 cargo run -p usage-app
```

Truthy values are `1`, `true`, `yes`, and `on`, case-insensitive and with
surrounding whitespace ignored. Any other value is off. On a debug build the
app writes one diagnostic to stderr on the first license evaluation and the
tray explicitly says `License: Pro (dev override)`. The local API reports
`"status":"pro"` and `"forced":true`, so scripts cannot mistake the shortcut
for a real activation.

## Check a running app

With UsageCheck running, execute:

```sh
./scripts/check-pro.sh
```

It probes `/v1/license`, `/v1/accounts`, and `/v1/usage` on
`http://127.0.0.1:${USAGECHECK_API_PORT:-5178}`. If
`USAGECHECK_API_TOKEN` is set, it sends it as a bearer token. The output is a
tab-separated `provider`, `account`, `status` table.

The script prints the number of configured Cursor, Grok, and Higgsfield
accounts it examined. It exits non-zero when the API cannot be reached or
when the license reports Pro but any such account still has the
`pro_required` status. With zero paid accounts it exits zero but explicitly
reports that Pro gating was not verified; otherwise, exit zero means no
active gating regression was found. It uses stock macOS `curl` and `python3`;
no `jq` installation is required.
