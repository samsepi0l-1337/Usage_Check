# UsageCheck License API — implementer's guide (Stage B)

This document specifies the HTTP contract and signed-token format the
`autoworkit.com` license server must implement for UsageCheck's runtime Pro
license gate. It is written for whoever implements the server side; the
client-side Rust implementation lives in `src-tauri/src/license/` (see
`http.rs` for the HTTP transport, `token.rs` for the token format, and
`pubkey.rs` for public-key handling).

## Why a signed token, not a plain "yes/no"

Earlier revisions cached a plain-JSON activation record on disk. A user could
hand-edit that file (`plan: "pro"`, their own device id, a fresh
`verified_at`) and get Pro forever — an online-only check cannot fix this,
because the *cached* "yes" is what the app actually trusts between checks.

The fix: **the server's answer is a signed token.** The app persists the
token verbatim and re-verifies its Ed25519 signature against an embedded
public key on **every** status evaluation. Editing the persisted file then
invalidates the signature, and the app fails closed (`Free`) — a forged
token is cryptographically indistinguishable from a missing one. The server
holds the **private** key; the app only ever embeds the **public** key.

## 1. HTTP contract

### Endpoint

```
POST https://autoworkit.com/api/license/verify
```

The client can point at a different host at runtime via the
`USAGECHECK_LICENSE_API` environment variable — used for local/staging
testing, and by the client's own mock-server test suite. Production ships
with the URL above as the compiled-in default.

### Request

```json
{
  "key": "<license key as the user typed it>",
  "device": "<64-char lowercase hex — see \"Device binding\" below>",
  "app_version": "<the UsageCheck crate version, e.g. \"0.1.33\">"
}
```

`Content-Type: application/json`, `Accept: application/json`.

### Success — `200 OK`

```json
{ "token": "<payload-segment>.<signature-segment>" }
```

See §2 for the exact token format.

### Failure — any non-2xx status

```json
{ "error": "<machine-readable code>", "message": "<human-readable text>" }
```

The client handles these `error` codes explicitly:

| `error` code    | Meaning                                              | Suggested HTTP status |
|-----------------|-------------------------------------------------------|------------------------|
| `invalid_key`   | The key does not exist / was never issued.            | 400 or 404             |
| `revoked`       | The key existed but has been revoked (refund, abuse).  | 403                    |
| `device_limit`  | The key has already been activated on its max devices. | 403                    |
| `expired`       | The key's subscription/term has already lapsed.        | 403                    |

Any OTHER `error` code is treated by the client as a generic failure whose
`message` is surfaced to the user verbatim — so it is safe to add new codes
later (forward-compatible) as long as `message` is still a reasonable
human-readable sentence.

A `404` or `405` from the **default** production host (i.e. the client was
NOT overridden via `USAGECHECK_LICENSE_API`) is treated specially by the
client as "the endpoint is not implemented yet" — useful while the server is
still being built, distinct from a real `invalid_key`/`revoked`/etc. failure.

### Timeouts and retries

The client uses a 10s connect timeout and a 15s total timeout per request,
and never automatically retries a failed request more than once. Design the
server so a legitimate request comfortably completes well inside 15s.

## 2. Token format

Compact, JWS-like, ASCII-safe, single line:

```
<base64url_nopad(payload_json)>.<base64url_nopad(ed25519_signature)>
```

- Both segments are **base64url, no padding** (RFC 4648 §5, `=` stripped).
- The **signature covers the RAW BYTES of the first segment's decoded
  payload JSON** — i.e. sign exactly the bytes you are about to base64url
  the payload from, and nothing else. The client verifies over those exact
  decoded bytes too, never over a re-serialized value, so there is no JSON
  canonicalization ambiguity between what the server signed and what the
  client verifies (whitespace, key order, and float formatting differences
  would otherwise silently break verification).

### Payload schema

All timestamps are RFC 3339 UTC (e.g. `"2026-07-27T12:00:00Z"`).

```json
{
  "v": 1,
  "key_id": "opaque id of the license the site issued",
  "plan": "pro",
  "device": "<the device id the client sent in the request>",
  "issued_at": "2026-07-27T12:00:00Z",
  "expires_at": "2027-07-27T12:00:00Z"
}
```

| Field        | Type             | Notes                                                                 |
|--------------|------------------|------------------------------------------------------------------------|
| `v`          | integer          | Payload schema version. The client currently only accepts `1`.        |
| `key_id`     | string           | Opaque identifier for the license (not the raw key). For diagnostics. |
| `plan`       | string           | Only `"pro"` currently unlocks anything client-side.                   |
| `device`     | string           | MUST echo the `device` field from the request verbatim.               |
| `issued_at`  | RFC 3339 string  | When this specific token was minted. Must not be in the future.       |
| `expires_at` | RFC 3339 string \| `null` | `null` = perpetual license. Otherwise the hard expiry date.  |

**Unknown extra fields must be tolerated** — the client ignores anything it
does not recognize, so the payload can grow new fields later without
breaking older client versions already in the field.

### Device binding

`device` is a 64-character lowercase-hex SHA-256 digest of a random UUID v4
the client generates once on first run and persists locally. It is:

- **Stable** across app restarts on the same install.
- **Not** derived from any hardware serial, MAC address, or other
  fingerprintable identifier — it contains no personal or hardware data.
- Bound into the signed token, so copying a `license.json` (or just the
  token string) to a different machine does not grant Pro there: the copied
  token's `device` field will not match the new machine's own device id, and
  the client refuses it (`Free`).
- The server should record which device id(s) a key has activated, in order
  to enforce `device_limit`.

### Reference implementation of the signature

Any Ed25519 library works; the client uses Rust's `ed25519-dalek` with
`verify_strict` (rejects malleable/non-canonical signatures). Server-side in
any language, the algorithm is:

1. Build the payload JSON object above (any valid JSON serialization).
2. Take the UTF-8 bytes of that JSON — call them `payload_bytes`.
3. `signature_bytes = Ed25519_Sign(private_key, payload_bytes)` (64 bytes).
4. `token = base64url_nopad(payload_bytes) + "." + base64url_nopad(signature_bytes)`.

## 3. Generating the keypair

```sh
# Generate a private key (PKCS#8 PEM).
openssl genpkey -algorithm ed25519 -out license-signing-key.pem

# Extract the matching public key.
openssl pkey -in license-signing-key.pem -pubout -out license-public-key.pem

# Extract the RAW 32-byte public key and base64-encode it — this is the
# value that gets embedded in the UsageCheck client build.
openssl pkey -in license-signing-key.pem -pubout -outform DER \
  | tail -c 32 | base64
```

**Keep `license-signing-key.pem` on the server only — it must never ship in
the client binary or any repository.** The client embeds ONLY the base64 of
the raw 32-byte public key printed by the last command above, as the
`EMBEDDED_PUBLIC_KEY_B64` constant in `src-tauri/src/license/pubkey.rs`. A
release build shipped with that constant still set to its documented
placeholder value fails closed (every token fails verification, so the app
never grants Pro) rather than silently trusting an unset key — replacing the
placeholder with the real key from this step is a required release step, not
optional hardening.

### Debug/staging override

For local development and CI, a **debug build only** may override the
embedded public key via the `USAGECHECK_LICENSE_PUBKEY` environment variable
(same base64-of-32-raw-bytes format). This lets a developer or a staging
deployment point verification at a throwaway test keypair without touching
the compiled-in constant. Release builds never read this variable — the
branch that would read it is compiled out entirely, so a release binary can
never be redirected to an attacker-controlled key via the environment.

## 4. Offline grace behavior

The client does not need to reach the server on every launch:

- **`issued_at` is the freshness anchor — not a client-side "last verified"
  timestamp.** The client persists `verified_at` (the time it locally
  observed a successful verification) purely for DISPLAY/diagnostics; it is
  plain JSON outside the token's signature, so it is trivially
  hand-editable and carries **zero** authority. The value that actually
  governs the offline grace period is the token's own SIGNED
  `payload.issued_at`. **The server MUST mint a fresh `issued_at` on every
  successful verify response** (activation AND refresh) — never reissue a
  token whose `issued_at` is unchanged from a previous response for the
  same device, and never reuse a cached/pre-signed token across requests.
- While `expires_at` (if set) has not passed, and while
  `now - issued_at <= 14 days`, the client treats the license as valid
  **Pro** without contacting the server again.
- Once `now - issued_at > 14 days`, the client shows the license as
  **grace-period-ended** — non-Pro — until the next successful online
  verification issues a token with a newer `issued_at`.
- **Replay detection:** the client compares a candidate token's `issued_at`
  against the STORED token's own `issued_at` and REJECTS the response
  outright (nothing is persisted — no license record, no watermark) unless
  the new value is STRICTLY newer. This applies to **both** `activate` and
  `refresh`, not just refresh — whichever entry point is called, if a
  record is already stored for this app-data directory and that stored
  token still verifies, the candidate must strictly advance past it. When
  no record is stored yet, or the stored one no longer verifies (e.g. it
  was already corrupt), there is no trustworthy prior to compare against,
  so the guard is skipped and the candidate goes through full validation
  instead. A server response that does not advance `issued_at` — a replay,
  a caching/proxy bug, or a server that accidentally re-signs the same
  payload bytes — is therefore treated as a replay attempt, not as "no
  change needed." Implementers should ensure every real verify response
  embeds a genuinely fresh timestamp, not a value quantized or cached in a
  way that could collide with a prior response for the same device.
- The client also attempts a background refresh automatically at most once
  every 24 hours while a license is active, so in normal operation the
  14-day grace window is rarely actually approached; it exists for
  legitimately offline stretches (travel, no network), not as the expected
  steady state.
- A **clock rollback** (the client's wall clock moved backward by more than
  ~5 minutes since the last time an online verification set the rollback
  watermark) is detected independently and forces the same
  grace-period-ended state, requiring a fresh online verification — rolling
  the clock back can never extend or restore an entitlement.
- **The rollback watermark's lifecycle follows one unifying rule: it is
  only ever ADVANCED by SERVER-AUTHENTICATED time — a freshly-signed
  token's own `issued_at` — and it is never created or repaired by a
  read/status path.**
  - **Fail-closed is PERSISTENT, not one-shot.** If the watermark file is
    missing, corrupted, or unwritable while a license record exists, the
    client treats the offline grace period as already exhausted (requiring
    a fresh online verification) on **every** status evaluation until that
    online verification actually succeeds — not just the first evaluation
    after the file was lost. A read/status check never writes the
    watermark file under any outcome.
  - **A watermark that regresses is also detected**, even when it is
    perfectly well-formed. The client persists the watermark together with
    the `issued_at` of the token that last advanced it; if the stored
    high-watermark ever predates the CURRENTLY-stored token's own signed
    `issued_at`, that is treated as tampering (the same
    grace-period-ended state) — a legitimate watermark can never fall
    behind the token it was set against. This closes the specific case of
    a user hand-editing the watermark file alone, without also rolling the
    system clock back or touching the (signed, unforgeable) token.
  - **Recovery is server-authenticated, and gated on the candidate actually
    resolving to Pro — not merely on signature/device verification
    succeeding.** The client evaluates the FULL `decide_status` rules
    (signature, `v`/`plan`, device, replay guard, and — against an
    in-memory candidate watermark, never the on-disk one — expiry and
    offline grace) BEFORE writing anything. Only once that evaluation
    resolves to `Pro` does it write anything at all — and even then, what
    happens is two ORDERED, SEPARATE file writes, not one atomic commit:
    the on-disk watermark is REPLACED with `max(now, that token's
    issued_at)` FIRST, and the license record is persisted SECOND,
    regardless of what was previously on disk (missing, corrupted, or a
    bogus far-future value). Both writes' failures are surfaced as an
    error rather than swallowed, and the record write is never even
    attempted once the watermark write has already failed. A candidate
    that verifies but is expired, future-issued, or otherwise rejected
    leaves the watermark (and the license record) completely untouched,
    exactly like a rejected candidate leaves the license record untouched.
    Because these are two separate filesystem operations, a crash or
    process kill strictly BETWEEN them is still possible — but only in the
    safe direction: watermark-present/record-absent reads back as
    ordinary `Free` (no record, no entitlement, harmless), never the
    reverse. A record can never be observed on disk without a watermark
    protecting it, because the watermark write always happens, and always
    succeeds, before the record write is attempted. This is safe
    specifically because the new value is anchored to a signature the
    client cannot forge, so a genuine online verification resolving to Pro
    is always more trustworthy than whatever local state it overwrites.
    This is the ONLY way an honest user whose clock was briefly wrong, or
    whose local state was corrupted, gets back to Pro — a read-only status
    check can never do it, by design.
- A **failed** background refresh attempt (network error, server
  unavailable) does **not** immediately downgrade the user — the 14-day
  grace period above is what governs entitlement between successful
  verifications, so a transient outage does not lock out a paying user.

## 5. Worked example (THROWAWAY test key — do not use in production)

The following was generated with a deliberately public, non-secret test
keypair, purely so the server team has one known-good vector to validate
their signer against. **This keypair must never be used for anything real.**

- Test private key (32-byte seed, hex, all `0x0b` bytes — THROWAWAY, public):
  `0b0b0b0b0b0b0b0b0b0b0b0b0b0b0b0b0b0b0b0b0b0b0b0b0b0b0b0b0b0b0b0b`
- Test public key (base64 of the raw 32-byte verifying key):
  `Zr5+Myx6RTMyvZ0Kf32wVfXF7xoGraZtmLOftoEMRzo=`
- Example payload (note: the device id below is a placeholder 64-zero
  string for readability — a real one is 64 lowercase hex characters, not
  all the same digit):
  ```json
  {
    "v": 1,
    "key_id": "test-key-id",
    "plan": "pro",
    "device": "0000000000000000000000000000000000000000000000000000000000000000",
    "issued_at": "2026-01-01T00:00:00Z",
    "expires_at": null
  }
  ```
- Example resulting token (payload segment . signature segment):
  ```
  eyJ2IjoxLCJrZXlfaWQiOiJ0ZXN0LWtleS1pZCIsInBsYW4iOiJwcm8iLCJkZXZpY2UiOiIwMDAwMDAwMDAwMDAwMDAwMDAwMDAwMDAwMDAwMDAwMDAwMDAwMDAwMDAwMDAwMDAwMDAwMDAwMDAwMDAwMDAwIiwiaXNzdWVkX2F0IjoiMjAyNi0wMS0wMVQwMDowMDowMFoiLCJleHBpcmVzX2F0IjpudWxsfQ.DDEAV_QZp5y1haEGFq-v1hoioIvhwOGdftYv_V8OWDs5fjpz9wzl0GqmASw2ckNemhou742GShFbRE3LhLCNAA
  ```

This exact vector is asserted against a real signature verification in the
client's own test suite
(`src-tauri/src/license/http_tests.rs::doc_worked_example_token_verifies_against_the_documented_pubkey`),
so it can never silently drift from what the client actually accepts.

To regenerate this vector yourself: build the JSON payload above exactly
(field order does not matter — only the bytes actually signed matter, and
those are whatever bytes YOUR JSON serializer produces, which is why this
doc emphasizes "sign exactly the bytes you decode" rather than a fixed byte
string), sign it with the 32-byte all-`0x0b` seed using any Ed25519 library,
and base64url (no padding) both segments.

## 6. Client-side surface (for reference — not the server's concern)

The Rust client exposes:

```rust
pub async fn activate(key: &str) -> Result<LicenseStatus, ActivationError>;
pub async fn refresh() -> Result<LicenseStatus, ActivationError>;
pub fn deactivate() -> Result<(), String>;
```

`activate` verifies the returned token's signature and payload BEFORE
persisting anything — a server response that fails verification for any
reason (bad signature, wrong `device`, unexpected `plan`/`v`, a replayed
`issued_at` against whatever record is already stored for this app-data
directory, a non-durable device id, or a candidate that does not evaluate to
`Pro` under the exact same rules `status()` itself uses) results in nothing
being written to disk — **neither `license.json` nor the rollback watermark
(`clock-watermark`)**; a previously-good record (and its previously-good
watermark) is never overwritten by a failed attempt. The full temporal/
watermark/rollback evaluation runs against an **in-memory** candidate
watermark before either file is touched, specifically so that a candidate
which fails that evaluation (expired, future-issued, still within its own
grace but otherwise rejected) cannot advance the on-disk watermark even
though the license record is correctly left alone. Once the candidate
resolves to `Pro`, the client writes the **watermark first, then the
license record** — deliberately in that order, and deliberately as two
separate writes rather than one atomic commit: a record published without a
protecting watermark is the unsafe half of the pair (§4 above already
treats a missing watermark as tampering/loss whenever a record exists), so
the watermark write happens, and must succeed, before the record write is
even attempted. Each write's failure is surfaced as an error
(`ActivationError::Persist`) instead of being swallowed. A crash or process
kill strictly between the two writes is still possible — they are two
separate filesystem operations, not one transaction — but it can only ever
leave a watermark with no record (reads back as ordinary `Free`, harmless),
never a published record with no watermark. **`activate` is subject to the
same replay guard as
`refresh`** — if a stored record already exists and its token still
verifies, `activate`'s candidate must strictly advance past its
`issued_at` too, not just `refresh`'s. `refresh` re-runs activation with the
previously-stored key, and is what the 24-hour background refresh calls.

Both `activate` and `refresh` serialize their entire read-validate-commit
sequence behind a single in-process lock, so two calls racing each other
(e.g. a manual "Activate" click landing at the same time as the periodic
background refresh) can never lose a newer, already-committed token to an
older response arriving late — whichever call runs second always re-reads
the other's fresh commit before validating its own candidate against it.

Before either function contacts the server at all, it requires this
machine's device id to be DURABLY persisted to disk — if persistence has
failed (e.g. an unwritable app-data directory), the call refuses outright
with no network request and nothing written, rather than risk binding a
freshly-issued token to a process-only id that a restart would silently
replace with a different one.

## 7. Threat model and accepted limits

This section states plainly what client-side enforcement can and cannot
guarantee, so nobody mistakes "the client fails closed on a bad token" for a
stronger property than it actually is.

- **Device binding is defeated by copying the whole app-data directory, not
  just the license file.** The device id is itself a value derived from a
  UUID persisted under the same app-data directory as the license record.
  An attacker (or a user) who copies the ENTIRE app-data directory to
  another machine copies the device id along with the license record, so
  the copied token's `device` field matches the new machine's own
  (also-copied) device id and verification succeeds there too. Copying just
  `license.json` alone does not work (the device id would not match), but
  copying the whole directory does. **The real control against this is
  server-side seat accounting** — the server should track how many distinct
  devices have activated a given key and enforce `device_limit`
  independently of anything the client reports; the client-side device
  binding narrows casual sharing of a single `license.json`, it does not
  replace server-side enforcement.
- **Symlink checks are metadata-check-then-open, not atomic — TOCTOU-prone
  against a LOCAL attacker.** `reject_symlink` inspects a path's metadata
  and then a separate operation reads/writes/removes it; a local attacker
  who can race the filesystem between those two steps (swap a real file for
  a symlink after the check, before the open) can in principle defeat the
  check. This is accepted: closing it fully requires atomic
  open-with-`O_NOFOLLOW`-style primitives throughout, which is a larger
  change than this hardening pass, and the threat model here is a
  non-privileged local user tampering with their own app-data directory —
  not a privileged/remote attacker, against whom this file-level guard was
  never a defense in the first place.
- **The rollback watermark is a BEST-EFFORT speed bump, not an
  authenticated anti-rollback mechanism — the client holds no secret, so
  nothing about the watermark file itself can ever be cryptographically
  verified.** Unlike the token, which is signed with a private key only the
  server holds, `clock-watermark` is plain, unsigned local state written by
  the same process that reads it. A local owner who controls BOTH the
  system clock and the app-data directory can still extend offline
  entitlement — well short of patching the binary — by hand-writing or
  copying a watermark file (including the degenerate case
  `max_seen == issued_at` — the client's own tampering check only rejects a
  watermark that falls STRICTLY BEHIND the stored token's `issued_at`, so a
  watermark exactly equal to it is indistinguishable from one the client
  produced itself) that the client cannot tell apart from a genuine one. The
  watermark/rollback machinery in §4 closes specific failure modes (a bare
  clock rollback, a hand-edited watermark file left otherwise alone, a
  candidate that fails validation advancing the watermark anyway) but does
  not, and cannot, make offline time itself trustworthy: an owner who
  freezes or slows their own clock, or who holds it and the watermark file
  together at a self-consistent value that never regresses past the
  currently-stored token's `issued_at`, can stretch the 14-day offline
  grace period indefinitely without ever touching the signed token or
  triggering rollback detection. This is **accepted**, not a bug — the
  token's signature is the only thing that is cryptographically protected
  here, and nothing about a local machine's wall clock, or a local file
  containing nothing but timestamps, ever carries that kind of guarantee.
  **The real controls against this are not client-side at all:**
  **periodic online verification** (the 24-hour background refresh, which
  a user who wants to stay offline indefinitely must actively keep
  suppressed) and **server-side seat/expiry accounting**, which does not
  depend on anything the client-side clock or watermark file reports.
  Nothing in this document should be read as claiming the watermark makes
  rollback cryptographically infeasible — it only raises the bar above a
  plain, unprotected "last verified" timestamp.
- **A local owner can ultimately patch the binary.** All client-side
  enforcement in this module — signature verification, device binding,
  offline grace, rollback detection — runs on a machine the license holder
  fully controls. Someone willing to patch the compiled binary (skip the
  signature check, hardcode `LicenseStatus::Pro`, etc.) can always do so;
  nothing described in this document is a cryptographic guarantee against
  the machine's own owner. What it actually buys is **friction against
  casual sharing/editing** (a hand-edited JSON file, a copied token, a
  rolled-back clock) — a meaningfully higher bar than a plain-JSON cache,
  but not an unbypassable one. Real revenue protection is the server's
  ability to revoke a key and refuse further verifications, not anything
  the client enforces locally.
- **The embedded public key currently ships as a PLACEHOLDER.**
  `src-tauri/src/license/pubkey.rs`'s `EMBEDDED_PUBLIC_KEY_B64` is derived
  from an all-zero signing key, not a real production key. A release build
  compiled from this tree therefore fails closed on every token
  (`resolve_public_key` returns `None` for a release build still on the
  placeholder — see that file) and is **permanently `Free`** until the
  placeholder is replaced with the real `autoworkit.com` production public
  key, per §3 above. This is intentional (fail closed, never silently trust
  an unset key) but is also a real, standing limitation of anything built
  from this tree today: Stage C is expected to add a CI assertion that a
  release build never ships with the placeholder still in place, so this
  cannot silently regress once the real key is embedded.
