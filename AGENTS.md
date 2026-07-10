## Learned User Preferences

- Build UsageCheck as a cross-platform Windows and macOS app that shows Codex, Claude Code, and Antigravity (agy) usage.
- Keep macOS and Windows UX as native tray menu only (Docker-style); do not ship a separate custom popup/usage window. Windows must hide the console and show usage from the notification-area tray icon click.
- For multi-account trays, show the account username on one line with usage on the next, and group rows by vendor category (Codex, Claude, Antigravity/agy) so each provider appears as one block. Agy shows Antigravity Model Quota remaining % for the Gemini and Claude+GPT pools (not local SQLite token totals).
- Prefer shipping shareable installers: macOS `.dmg` and Windows `.exe` (NSIS), not source trees or the legacy Swift package.
- On macOS, install from the DMG into `/Applications` only; do not treat repo build `.app` trees under `.build/`, `target/`, or `dist/ci/` as the installed app (Launchpad indexes those and shows duplicates).

## Learned Workspace Facts

- UsageCheck is a Tauri v2 tray shell (`src-tauri` / `usage-app`) over a shared Rust core (`crates/usage-core`) for provider models, API fetchers, and local log scanning.
- Codex and Claude usage come from each provider’s usage API (5-hour and 7-day quotas). Agy uses Antigravity `RetrieveUserQuotaSummary` (prefer running Antigravity.app language_server; else Google OAuth → Cloud Code), showing remaining % for Gemini Models and Claude and GPT models pools.
- The original Swift macOS app under `Sources/` / `Package.swift` is reference-only and is not part of the cross-platform rewrite.
- The Vite UI under `ui/` is legacy and unused by the tray-menu shell; tray builds only need a minimal `ui/dist` placeholder for Tauri bundling.
- The usual Mac handoff artifact is a `.dmg` (for example under `dist/` or `src-tauri/target/release/bundle/dmg/`); Windows installers are produced on Windows or via the GitHub Release workflow.
- macOS release/CI bundles need `bundle.macOS.signingIdentity: "-"` so Tauri ad-hoc-signs the `.app` (linker-signed-only bundles are rejected by Gatekeeper as “damaged”); CI should `codesign --verify` and fail on `linker-signed`. Without Apple Developer ID/notarization, first open may still need Privacy & Security → Open Anyway or `xattr -cr` on the app.
