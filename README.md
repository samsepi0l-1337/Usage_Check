# UsageCheck

macOS menu bar usage monitor for Codex, Claude Code, and Gemini-family local logs.

## What It Shows

- Codex: overall pool plus `gpt-5.3-codex-spark` pool.
- Claude Code: overall pool plus Sonnet pool.
- Gemini: Gemini-model pool plus non-Gemini model pool.
- Each pool shows 5-hour, 7-day, and 30-day token usage.
- Codex and Claude overall pools also show API quota percentages when local OAuth credentials are available.

The app reads local files only, except for the provider API calls used to fetch Codex and Claude quota percentages:

- Codex logs: `~/.codex/sessions`, `~/.codex/archived_sessions`
- Codex auth: `~/.codex/auth.json`, or `CODEX_HOME/auth.json`
- Claude logs: `~/.claude/projects`, `~/.config/claude/projects`, or `CLAUDE_CONFIG_DIR`
- Claude auth: `~/.claude/.credentials.json`
- Gemini logs: `~/.gemini/**/transcript*.jsonl`, including Antigravity CLI transcripts

## Run

```sh
swift run UsageCheck
```

The app runs as a menu bar item. Click the chart icon in the top macOS menu bar to open the usage menu.

## Build An App Bundle

```sh
./scripts/package-app.sh
open .build/release/UsageCheck.app
```

## Verify

```sh
swift test
swift build -c release
```
