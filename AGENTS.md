# AGENTS.md

## Learned User Preferences

- Build UsageCheck as a cross-platform Windows and macOS app that shows Codex, Claude Code, and Antigravity (agy) usage.
- Keep macOS and Windows UX as native tray menu only (Docker-style); do not ship a separate custom popup/usage window. Windows must hide the console and show usage from the notification-area tray icon click.
- For multi-account trays, show the account username on one line with usage on the next, and group rows by vendor category (Codex, Claude, Antigravity/agy) so each provider appears as one block. Agy shows Antigravity Model Quota as used % (0→100, like Codex/Claude) for the Gemini and Claude+GPT pools (not local SQLite token totals).
- Prefer shipping shareable installers: macOS `.dmg` and Windows `.exe` (NSIS), not source trees or the legacy Swift package.
- Ship **two edition binaries** (Free and Pro), not a runtime license unlock: `UsageCheck-Free` (`com.usagecheck.desktop.free`) and `UsageCheck-Pro` (`com.usagecheck.desktop.pro`). Build with `./scripts/build-edition.sh free|pro`.
- On macOS, install from the DMG into `/Applications` only; do not treat repo build `.app` trees under `.build/`, `target/`, or `dist/ci/` as the installed app (Launchpad indexes those and shows duplicates).
- Expose usage over a local OpenAPI HTTP API so other coding agents (Codex, Claude Code, agy, Cursor) can wrap it as MCP servers or skills.
- Keep browser-imported multi-account credentials stable when the user later runs CLI `login` for another account; apply the same session-isolation fix across Codex, Claude, and Agy—not one provider only.
- For Pro provider auth, prefer simple import paths (Grok clipboard/env Management Key; Cursor local DB; Higgsfield CLI/tray browser) over heavier custom OAuth when a simpler path exists.

## Learned Workspace Facts

- UsageCheck is a Tauri v2 tray shell (`src-tauri` / `usage-app`) over a shared Rust core (`crates/usage-core`) for provider models, API fetchers, and local log scanning.
- Codex and Claude usage come from each provider's usage API (5-hour and 7-day quotas). Agy uses Antigravity `RetrieveUserQuotaSummary` (prefer running Antigravity.app `language_server`; else Google OAuth → Cloud Code), converting `remainingFraction` to used % for Gemini Models and Claude and GPT models pools. Agy account add is browser OAuth only (the tray “Add Antigravity (running app)” path was removed).
- The original Swift macOS app under `Sources/` / `Package.swift` is reference-only and is not part of the cross-platform rewrite.
- The Vite UI under `ui/` is legacy and unused by the tray-menu shell; tray builds only need a minimal `ui/dist` placeholder for Tauri bundling.
- The usual Mac handoff artifact is a `.dmg` (for example under `dist/` or `src-tauri/target/release/bundle/dmg/`); Windows installers are produced on Windows or via the GitHub Release workflow.
- macOS release/CI bundles need `bundle.macOS.signingIdentity: "-"` so Tauri ad-hoc-signs the `.app` (linker-signed-only bundles are rejected by Gatekeeper as "damaged"); CI should `codesign --verify` and fail on `linker-signed`. Without Apple Developer ID/notarization, first open may still need Privacy & Security → Open Anyway or `xattr -cr` on the app.
- **Free edition** (default `edition-free`): Codex, Claude, agy (Gemini via Antigravity quota API). **Pro edition** (`edition-pro`): adds Cursor (local `state.vscdb` + undocumented `GetCurrentPeriodUsage`), Grok (xAI Management prepaid via clipboard/env `XAI_MGMT_KEY`/`XAI_TEAM_ID`), Higgsfield (CLI `account --json` / tray browser login). No online license server. CI Release workflow builds a 4-job matrix (`UsageCheck-Free-macos`, `UsageCheck-Pro-macos`, `UsageCheck-Free-windows`, `UsageCheck-Pro-windows`) on `v*` tags. See `docs/editions.md`.
- While the tray app is running, a local usage API listens on `127.0.0.1:5178` (override with `USAGECHECK_API_PORT`): `GET /v1/usage`, `GET /v1/usage/{provider}`, `GET /openapi.yaml`. Spec lives at `docs/openapi.yaml`; intended for MCP/skill wrappers.
