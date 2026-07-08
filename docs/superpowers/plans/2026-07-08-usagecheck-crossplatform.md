# UsageCheck Cross-Platform Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Build a Tauri (Rust core + web UI) app that shows Codex, Claude Code, and agy usage (5-hour and weekly quota) in the macOS menu bar and Windows taskbar tray, with multi-account support per provider.

**Architecture:** A pure-logic Rust library crate (`usage-core`) holds models, aggregation, log scanning, and provider fetchers — all unit-tested. A Tauri binary crate (`usage-app`) hosts the tray icon, a borderless popup WebView, account storage in the OS keychain, an OAuth (PKCE) login manager, and a polling orchestrator that emits `UsageSnapshot` to the web UI. The existing Swift app is kept as reference; its logic is ported 1:1 to Rust.

**Tech Stack:** Rust 2021, Tauri v2, `reqwest` (HTTP), `serde`/`serde_json`, `keyring` (OS keychain), `tiny_http` (localhost OAuth callback), `chrono` (time), `open` (browser), TypeScript + Vite (web UI).

## Global Constraints

- Platforms: macOS 13+ and Windows 10+ (x64). Single Tauri codebase.
- Existing Swift sources under `Sources/` are reference-only — do NOT delete or modify them.
- All new Rust code lives under `src-tauri/` (Tauri convention); shared logic in the `usage-core` crate within a Cargo workspace.
- Credentials are stored ONLY in the OS keychain via `keyring`; never written to plaintext files by this app.
- Provider API endpoints (verbatim from spec):
  - Codex: `https://chatgpt.com/backend-api/wham/usage`
  - Claude: `https://api.anthropic.com/api/oauth/usage`
- Usage windows: five-hours = 5*60*60s, week = 7*24*60*60s, month = 30*24*60*60s.
- The tray click opens a borderless popup; per-account cards show current 5-hour % and weekly 7-day % gauges plus reset time.
- TDD: every Rust logic task writes a failing test first. Commit after each task.
- agy/Gemini: no confirmed quota-% API — display best-effort local-log token aggregation (Task 9 investigates and confirms).

---

## File Structure

```
Cargo.toml                     # workspace root (new)
crates/usage-core/
  Cargo.toml
  src/lib.rs                   # re-exports
  src/models.rs                # UsageWindow, WindowTotals, QuotaUsage, PoolUsage, ...
  src/aggregate.rs             # fold ModelTokenEvent -> WindowTotals
  src/scanners/codex.rs        # ~/.codex/sessions JSONL parsing
  src/scanners/claude.rs       # ~/.claude/projects JSONL parsing
  src/scanners/gemini.rs       # ~/.gemini transcript parsing
  src/fetch/codex.rs           # parse wham/usage JSON -> QuotaUsage
  src/fetch/claude.rs          # parse oauth/usage JSON -> QuotaUsage
  src/account.rs               # Account, Provider, AccountId types
src-tauri/
  Cargo.toml
  tauri.conf.json
  src/main.rs                  # Tauri entry, tray, commands
  src/store.rs                 # keychain-backed AccountStore
  src/oauth.rs                 # PKCE + localhost callback
  src/poller.rs                # periodic fetch orchestration
ui/
  index.html
  src/main.ts                  # render snapshot, add-account flow
  src/api.ts                   # Tauri invoke wrappers
  package.json / vite.config.ts
```

---

## Phase 0 — Scaffolding

### Task 0: Cargo workspace + usage-core crate skeleton

**Files:**
- Create: `Cargo.toml` (workspace)
- Create: `crates/usage-core/Cargo.toml`
- Create: `crates/usage-core/src/lib.rs`

**Interfaces:**
- Produces: a compilable `usage-core` library crate the rest of the plan extends.

- [ ] **Step 1: Create workspace root `Cargo.toml`**

```toml
[workspace]
members = ["crates/usage-core", "src-tauri"]
resolver = "2"
```

- [ ] **Step 2: Create `crates/usage-core/Cargo.toml`**

```toml
[package]
name = "usage-core"
version = "0.1.0"
edition = "2021"

[dependencies]
serde = { version = "1", features = ["derive"] }
serde_json = "1"
chrono = { version = "0.4", features = ["serde"] }
```

- [ ] **Step 3: Create `crates/usage-core/src/lib.rs`**

```rust
pub mod models;
```

- [ ] **Step 4: Verify it builds**

Run: `cargo build -p usage-core`
Expected: compiles (empty `models` module added in Task 1; for now create `src/models.rs` with `// placeholder` OR defer this build check to Task 1).

- [ ] **Step 5: Commit**

```bash
git add Cargo.toml crates/usage-core
git commit -m "chore: scaffold usage-core workspace crate"
```

---

## Phase 1 — Core models & aggregation (pure logic, TDD)

### Task 1: Usage models ported from Swift

**Files:**
- Create: `crates/usage-core/src/models.rs`
- Test: inline `#[cfg(test)]` in `models.rs`

**Interfaces:**
- Produces:
  - `enum UsageWindow { FiveHours, Week, Month }` with `fn duration_secs(&self) -> i64` and `fn title(&self) -> &'static str`.
  - `struct WindowTotals { pub five_hours: i64, pub week: i64, pub month: i64 }` with `fn add(&mut self, tokens: i64, timestamp: DateTime<Utc>, now: DateTime<Utc>)` and `fn get(&self, w: UsageWindow) -> i64`.
  - `struct QuotaUsage { pub percent: f64, pub resets_at: Option<DateTime<Utc>>, pub window_seconds: Option<i64> }`.
  - `struct ModelTokenEvent { pub timestamp: DateTime<Utc>, pub model: String, pub tokens: i64, pub dedupe_key: Option<String> }`.

- [ ] **Step 1: Write failing test**

```rust
#[cfg(test)]
mod tests {
    use super::*;
    use chrono::{Duration, Utc};

    #[test]
    fn add_buckets_by_age() {
        let now = Utc::now();
        let mut t = WindowTotals::default();
        t.add(10, now - Duration::hours(1), now);   // within 5h
        t.add(20, now - Duration::days(3), now);    // within week, not 5h
        t.add(40, now - Duration::days(20), now);   // within month only
        t.add(80, now - Duration::days(40), now);   // outside all
        assert_eq!(t.get(UsageWindow::FiveHours), 10);
        assert_eq!(t.get(UsageWindow::Week), 30);
        assert_eq!(t.get(UsageWindow::Month), 70);
    }

    #[test]
    fn add_ignores_nonpositive_and_future() {
        let now = Utc::now();
        let mut t = WindowTotals::default();
        t.add(0, now, now);
        t.add(-5, now, now);
        t.add(10, now + Duration::hours(1), now); // future
        assert_eq!(t.get(UsageWindow::Month), 0);
    }
}
```

- [ ] **Step 2: Run test to verify it fails**

Run: `cargo test -p usage-core`
Expected: FAIL (types not defined).

- [ ] **Step 3: Implement `models.rs`**

```rust
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum UsageWindow { FiveHours, Week, Month }

impl UsageWindow {
    pub fn duration_secs(&self) -> i64 {
        match self {
            UsageWindow::FiveHours => 5 * 60 * 60,
            UsageWindow::Week => 7 * 24 * 60 * 60,
            UsageWindow::Month => 30 * 24 * 60 * 60,
        }
    }
    pub fn title(&self) -> &'static str {
        match self {
            UsageWindow::FiveHours => "5h",
            UsageWindow::Week => "7d",
            UsageWindow::Month => "30d",
        }
    }
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct WindowTotals {
    pub five_hours: i64,
    pub week: i64,
    pub month: i64,
}

impl WindowTotals {
    pub fn add(&mut self, tokens: i64, timestamp: DateTime<Utc>, now: DateTime<Utc>) {
        if tokens <= 0 { return; }
        let age = now.signed_duration_since(timestamp).num_seconds();
        if age < 0 { return; }
        if age <= UsageWindow::Month.duration_secs() { self.month += tokens; }
        if age <= UsageWindow::Week.duration_secs() { self.week += tokens; }
        if age <= UsageWindow::FiveHours.duration_secs() { self.five_hours += tokens; }
    }
    pub fn get(&self, w: UsageWindow) -> i64 {
        match w {
            UsageWindow::FiveHours => self.five_hours,
            UsageWindow::Week => self.week,
            UsageWindow::Month => self.month,
        }
    }
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct QuotaUsage {
    pub percent: f64,
    pub resets_at: Option<DateTime<Utc>>,
    pub window_seconds: Option<i64>,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct ModelTokenEvent {
    pub timestamp: DateTime<Utc>,
    pub model: String,
    pub tokens: i64,
    pub dedupe_key: Option<String>,
}
```

- [ ] **Step 4: Run test to verify it passes**

Run: `cargo test -p usage-core`
Expected: PASS (both tests).

- [ ] **Step 5: Commit**

```bash
git add crates/usage-core/src/models.rs
git commit -m "feat(core): usage window models ported from Swift"
```

### Task 2: Aggregator with dedupe

**Files:**
- Create: `crates/usage-core/src/aggregate.rs`
- Modify: `crates/usage-core/src/lib.rs` (add `pub mod aggregate;`)

**Interfaces:**
- Consumes: `ModelTokenEvent`, `WindowTotals` from `models`.
- Produces: `fn aggregate(events: &[ModelTokenEvent], now: DateTime<Utc>) -> WindowTotals` — sums token events into windows, skipping events whose `dedupe_key` was already seen.

- [ ] **Step 1: Write failing test**

```rust
#[cfg(test)]
mod tests {
    use super::*;
    use crate::models::{ModelTokenEvent, UsageWindow};
    use chrono::{Duration, Utc};

    fn ev(mins: i64, tokens: i64, key: Option<&str>) -> ModelTokenEvent {
        ModelTokenEvent {
            timestamp: Utc::now() - Duration::minutes(mins),
            model: "m".into(),
            tokens,
            dedupe_key: key.map(|k| k.to_string()),
        }
    }

    #[test]
    fn sums_and_dedupes() {
        let now = Utc::now();
        let events = vec![ev(1, 10, Some("a")), ev(2, 10, Some("a")), ev(3, 5, None)];
        let t = aggregate(&events, now);
        assert_eq!(t.get(UsageWindow::FiveHours), 15); // second "a" skipped
    }
}
```

- [ ] **Step 2: Run to verify fail**

Run: `cargo test -p usage-core aggregate`
Expected: FAIL (function missing).

- [ ] **Step 3: Implement `aggregate.rs`**

```rust
use std::collections::HashSet;
use chrono::{DateTime, Utc};
use crate::models::{ModelTokenEvent, WindowTotals};

pub fn aggregate(events: &[ModelTokenEvent], now: DateTime<Utc>) -> WindowTotals {
    let mut totals = WindowTotals::default();
    let mut seen: HashSet<&str> = HashSet::new();
    for e in events {
        if let Some(key) = e.dedupe_key.as_deref() {
            if !seen.insert(key) { continue; }
        }
        totals.add(e.tokens, e.timestamp, now);
    }
    totals
}
```

Add `pub mod aggregate;` to `lib.rs`.

- [ ] **Step 4: Run to verify pass**

Run: `cargo test -p usage-core aggregate`
Expected: PASS.

- [ ] **Step 5: Commit**

```bash
git add crates/usage-core/src/aggregate.rs crates/usage-core/src/lib.rs
git commit -m "feat(core): token aggregation with dedupe"
```

---

## Phase 2 — Provider response parsing (pure, TDD)

> HTTP is a thin wrapper (Task 8); the JSON→`QuotaUsage` parsing is pure and lives here so it is fully unit-tested against fixtures. Fixtures mirror the shapes read in the existing Swift `OAuthFetchers.swift`.

### Task 3: Codex usage-response parser

**Files:**
- Create: `crates/usage-core/src/fetch/mod.rs` (`pub mod codex; pub mod claude;`)
- Create: `crates/usage-core/src/fetch/codex.rs`
- Modify: `lib.rs` (add `pub mod fetch;`)

**Interfaces:**
- Consumes: `QuotaUsage`.
- Produces: `struct CodexQuota { pub plan: Option<String>, pub five_hour: Option<QuotaUsage>, pub week: Option<QuotaUsage> }` and `fn parse_codex_usage(json: &serde_json::Value) -> CodexQuota`. Reads `rate_limit.primary_window` (5h) and `rate_limit.secondary_window` (week); each window has `used_percent`, `reset_at` (unix secs), `limit_window_seconds`.

- [ ] **Step 1: Write failing test**

```rust
#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn parses_primary_and_secondary() {
        let v = json!({
            "plan_type": "pro",
            "rate_limit": {
                "primary_window": {"used_percent": 42.5, "reset_at": 1_900_000_000.0, "limit_window_seconds": 18000},
                "secondary_window": {"used_percent": 12.0}
            }
        });
        let q = parse_codex_usage(&v);
        assert_eq!(q.plan.as_deref(), Some("pro"));
        assert_eq!(q.five_hour.as_ref().unwrap().percent, 42.5);
        assert_eq!(q.five_hour.as_ref().unwrap().window_seconds, Some(18000));
        assert!(q.five_hour.as_ref().unwrap().resets_at.is_some());
        assert_eq!(q.week.as_ref().unwrap().percent, 12.0);
        assert!(q.week.as_ref().unwrap().window_seconds.is_none());
    }

    #[test]
    fn missing_percent_yields_none() {
        let v = serde_json::json!({"rate_limit": {"primary_window": {}}});
        let q = parse_codex_usage(&v);
        assert!(q.five_hour.is_none());
    }
}
```

- [ ] **Step 2: Run to verify fail**

Run: `cargo test -p usage-core codex`
Expected: FAIL.

- [ ] **Step 3: Implement `fetch/codex.rs`**

```rust
use chrono::{TimeZone, Utc};
use serde_json::Value;
use crate::models::QuotaUsage;

pub struct CodexQuota {
    pub plan: Option<String>,
    pub five_hour: Option<QuotaUsage>,
    pub week: Option<QuotaUsage>,
}

fn window(v: &Value) -> Option<QuotaUsage> {
    let percent = v.get("used_percent")?.as_f64()?;
    let resets_at = v.get("reset_at").and_then(|x| x.as_f64())
        .and_then(|s| Utc.timestamp_opt(s as i64, 0).single());
    let window_seconds = v.get("limit_window_seconds").and_then(|x| x.as_i64())
        .filter(|s| *s > 0);
    Some(QuotaUsage { percent, resets_at, window_seconds })
}

pub fn parse_codex_usage(root: &Value) -> CodexQuota {
    let rl = root.get("rate_limit");
    let get = |k: &str| rl.and_then(|r| r.get(k)).and_then(window);
    CodexQuota {
        plan: root.get("plan_type").and_then(|x| x.as_str()).map(String::from),
        five_hour: get("primary_window"),
        week: get("secondary_window"),
    }
}
```

Create `fetch/mod.rs` with `pub mod codex;` and add `pub mod fetch;` to `lib.rs`.

- [ ] **Step 4: Run to verify pass**

Run: `cargo test -p usage-core codex`
Expected: PASS.

- [ ] **Step 5: Commit**

```bash
git add crates/usage-core/src/fetch crates/usage-core/src/lib.rs
git commit -m "feat(core): parse Codex wham/usage response"
```

### Task 4: Claude usage-response parser

**Files:**
- Create: `crates/usage-core/src/fetch/claude.rs`
- Modify: `crates/usage-core/src/fetch/mod.rs` (add `pub mod claude;`)

**Interfaces:**
- Produces: `struct ClaudeQuota { pub five_hour: Option<QuotaUsage>, pub week: Option<QuotaUsage> }` and `fn parse_claude_usage(json: &serde_json::Value) -> ClaudeQuota`. Reads `five_hour.utilization` and `seven_day.utilization` (percent), plus `resets_at` (RFC3339 or unix).

- [ ] **Step 1: Write failing test**

```rust
#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn parses_five_hour_and_seven_day() {
        let v = json!({
            "five_hour": {"utilization": 30.0, "resets_at": "2026-07-08T12:00:00Z"},
            "seven_day": {"utilization": 55.5}
        });
        let q = parse_claude_usage(&v);
        assert_eq!(q.five_hour.as_ref().unwrap().percent, 30.0);
        assert!(q.five_hour.as_ref().unwrap().resets_at.is_some());
        assert_eq!(q.week.as_ref().unwrap().percent, 55.5);
    }
}
```

- [ ] **Step 2: Run to verify fail**

Run: `cargo test -p usage-core claude`
Expected: FAIL.

- [ ] **Step 3: Implement `fetch/claude.rs`**

```rust
use chrono::{DateTime, Utc};
use serde_json::Value;
use crate::models::QuotaUsage;

pub struct ClaudeQuota {
    pub five_hour: Option<QuotaUsage>,
    pub week: Option<QuotaUsage>,
}

fn parse_resets_at(v: &Value) -> Option<DateTime<Utc>> {
    match v.get("resets_at") {
        Some(Value::String(s)) => DateTime::parse_from_rfc3339(s).ok().map(|d| d.with_timezone(&Utc)),
        Some(Value::Number(n)) => n.as_f64()
            .and_then(|s| chrono::TimeZone::timestamp_opt(&Utc, s as i64, 0).single()),
        _ => None,
    }
}

fn window(v: &Value) -> Option<QuotaUsage> {
    let percent = v.get("utilization")?.as_f64()?;
    Some(QuotaUsage { percent, resets_at: parse_resets_at(v), window_seconds: None })
}

pub fn parse_claude_usage(root: &Value) -> ClaudeQuota {
    ClaudeQuota {
        five_hour: root.get("five_hour").and_then(window),
        week: root.get("seven_day").and_then(window),
    }
}
```

Add `pub mod claude;` to `fetch/mod.rs`.

- [ ] **Step 4: Run to verify pass**

Run: `cargo test -p usage-core claude`
Expected: PASS.

- [ ] **Step 5: Commit**

```bash
git add crates/usage-core/src/fetch
git commit -m "feat(core): parse Claude oauth/usage response"
```

---

## Phase 3 — Local log scanners (pure, TDD)

### Task 5: Codex JSONL session scanner

**Files:**
- Create: `crates/usage-core/src/scanners/mod.rs` (`pub mod codex;`)
- Create: `crates/usage-core/src/scanners/codex.rs`
- Modify: `lib.rs` (add `pub mod scanners;`)

**Interfaces:**
- Consumes: `ModelTokenEvent`.
- Produces: `fn parse_codex_line(line: &str) -> Option<ModelTokenEvent>` — parses one JSONL record from `~/.codex/sessions/**/*.jsonl`. A token record has `timestamp` (RFC3339), `payload.info.total_token_usage.total_tokens` (i64), and `payload.model`. Also `fn latest_remaining_percent(line: &str) -> Option<f64>` reading a `remaining_percent` field for the token-free fallback (mirrors reference project).

- [ ] **Step 1: Write failing test**

```rust
#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_token_usage_line() {
        let line = r#"{"timestamp":"2026-07-08T10:00:00Z","payload":{"model":"gpt-5.3-codex-spark","info":{"total_token_usage":{"total_tokens":1234}}}}"#;
        let e = parse_codex_line(line).unwrap();
        assert_eq!(e.tokens, 1234);
        assert_eq!(e.model, "gpt-5.3-codex-spark");
    }

    #[test]
    fn ignores_non_token_line() {
        assert!(parse_codex_line(r#"{"timestamp":"2026-07-08T10:00:00Z","payload":{"type":"noise"}}"#).is_none());
    }

    #[test]
    fn reads_remaining_percent() {
        let line = r#"{"rate_limit":{"primary":{"remaining_percent":73.0}}}"#;
        assert_eq!(latest_remaining_percent(line), Some(73.0));
    }
}
```

- [ ] **Step 2: Run to verify fail**

Run: `cargo test -p usage-core scanners::codex`
Expected: FAIL.

- [ ] **Step 3: Implement `scanners/codex.rs`**

```rust
use chrono::{DateTime, Utc};
use serde_json::Value;
use crate::models::ModelTokenEvent;

pub fn parse_codex_line(line: &str) -> Option<ModelTokenEvent> {
    let v: Value = serde_json::from_str(line).ok()?;
    let ts = DateTime::parse_from_rfc3339(v.get("timestamp")?.as_str()?)
        .ok()?.with_timezone(&Utc);
    let payload = v.get("payload")?;
    let tokens = payload.pointer("/info/total_token_usage/total_tokens")?.as_i64()?;
    let model = payload.get("model").and_then(|m| m.as_str()).unwrap_or("").to_string();
    Some(ModelTokenEvent { timestamp: ts, model, tokens, dedupe_key: None })
}

pub fn latest_remaining_percent(line: &str) -> Option<f64> {
    let v: Value = serde_json::from_str(line).ok()?;
    v.pointer("/rate_limit/primary/remaining_percent").and_then(|x| x.as_f64())
}
```

Create `scanners/mod.rs` with `pub mod codex;` and add `pub mod scanners;` to `lib.rs`.

- [ ] **Step 4: Run to verify pass**

Run: `cargo test -p usage-core scanners::codex`
Expected: PASS.

- [ ] **Step 5: Commit**

```bash
git add crates/usage-core/src/scanners crates/usage-core/src/lib.rs
git commit -m "feat(core): Codex JSONL session scanner"
```

### Task 6: Claude JSONL project scanner

**Files:**
- Create: `crates/usage-core/src/scanners/claude.rs`
- Modify: `scanners/mod.rs` (add `pub mod claude;`)

**Interfaces:**
- Produces: `fn parse_claude_line(line: &str) -> Option<ModelTokenEvent>` — reads `~/.claude/projects/**/*.jsonl` assistant records: `timestamp` (RFC3339), `message.usage.{input_tokens,output_tokens,cache_creation_input_tokens,cache_read_input_tokens}` summed, `message.model`, and `message.id` as `dedupe_key`.

- [ ] **Step 1: Write failing test**

```rust
#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sums_usage_fields_and_sets_dedupe() {
        let line = r#"{"timestamp":"2026-07-08T10:00:00Z","message":{"id":"msg_1","model":"claude-sonnet-5","usage":{"input_tokens":10,"output_tokens":20,"cache_read_input_tokens":5}}}"#;
        let e = parse_claude_line(line).unwrap();
        assert_eq!(e.tokens, 35);
        assert_eq!(e.dedupe_key.as_deref(), Some("msg_1"));
        assert_eq!(e.model, "claude-sonnet-5");
    }

    #[test]
    fn ignores_line_without_usage() {
        assert!(parse_claude_line(r#"{"timestamp":"2026-07-08T10:00:00Z","message":{"role":"user"}}"#).is_none());
    }
}
```

- [ ] **Step 2: Run to verify fail**

Run: `cargo test -p usage-core scanners::claude`
Expected: FAIL.

- [ ] **Step 3: Implement `scanners/claude.rs`**

```rust
use chrono::{DateTime, Utc};
use serde_json::Value;
use crate::models::ModelTokenEvent;

pub fn parse_claude_line(line: &str) -> Option<ModelTokenEvent> {
    let v: Value = serde_json::from_str(line).ok()?;
    let ts = DateTime::parse_from_rfc3339(v.get("timestamp")?.as_str()?)
        .ok()?.with_timezone(&Utc);
    let msg = v.get("message")?;
    let usage = msg.get("usage")?;
    let field = |k: &str| usage.get(k).and_then(|x| x.as_i64()).unwrap_or(0);
    let tokens = field("input_tokens") + field("output_tokens")
        + field("cache_creation_input_tokens") + field("cache_read_input_tokens");
    if tokens <= 0 { return None; }
    let model = msg.get("model").and_then(|m| m.as_str()).unwrap_or("").to_string();
    let dedupe_key = msg.get("id").and_then(|i| i.as_str()).map(String::from);
    Some(ModelTokenEvent { timestamp: ts, model, tokens, dedupe_key })
}
```

Add `pub mod claude;` to `scanners/mod.rs`.

- [ ] **Step 4: Run to verify pass**

Run: `cargo test -p usage-core scanners::claude`
Expected: PASS.

- [ ] **Step 5: Commit**

```bash
git add crates/usage-core/src/scanners
git commit -m "feat(core): Claude JSONL project scanner"
```

### Task 7: Gemini/agy transcript scanner

**Files:**
- Create: `crates/usage-core/src/scanners/gemini.rs`
- Modify: `scanners/mod.rs` (add `pub mod gemini;`)

**Interfaces:**
- Produces: `fn parse_gemini_line(line: &str) -> Option<ModelTokenEvent>` — reads `~/.gemini/**/transcript*.jsonl` records: `timestamp` (RFC3339 or unix ms), `usageMetadata.totalTokenCount`, `model` (default "gemini").

- [ ] **Step 1: Write failing test**

```rust
#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_total_token_count() {
        let line = r#"{"timestamp":"2026-07-08T10:00:00Z","model":"gemini-3.5-flash","usageMetadata":{"totalTokenCount":900}}"#;
        let e = parse_gemini_line(line).unwrap();
        assert_eq!(e.tokens, 900);
        assert_eq!(e.model, "gemini-3.5-flash");
    }
}
```

- [ ] **Step 2: Run to verify fail**

Run: `cargo test -p usage-core scanners::gemini`
Expected: FAIL.

- [ ] **Step 3: Implement `scanners/gemini.rs`**

```rust
use chrono::{DateTime, TimeZone, Utc};
use serde_json::Value;
use crate::models::ModelTokenEvent;

fn parse_ts(v: &Value) -> Option<DateTime<Utc>> {
    match v.get("timestamp")? {
        Value::String(s) => DateTime::parse_from_rfc3339(s).ok().map(|d| d.with_timezone(&Utc)),
        Value::Number(n) => {
            let ms = n.as_f64()?;
            let secs = if ms > 10_000_000_000.0 { ms / 1000.0 } else { ms };
            Utc.timestamp_opt(secs as i64, 0).single()
        }
        _ => None,
    }
}

pub fn parse_gemini_line(line: &str) -> Option<ModelTokenEvent> {
    let v: Value = serde_json::from_str(line).ok()?;
    let ts = parse_ts(&v)?;
    let tokens = v.pointer("/usageMetadata/totalTokenCount")?.as_i64()?;
    if tokens <= 0 { return None; }
    let model = v.get("model").and_then(|m| m.as_str()).unwrap_or("gemini").to_string();
    Some(ModelTokenEvent { timestamp: ts, model, tokens, dedupe_key: None })
}
```

Add `pub mod gemini;` to `scanners/mod.rs`.

- [ ] **Step 4: Run to verify pass**

Run: `cargo test -p usage-core scanners::gemini`
Expected: PASS.

- [ ] **Step 5: Commit**

```bash
git add crates/usage-core/src/scanners
git commit -m "feat(core): Gemini/agy transcript scanner"
```

---

## Phase 4 — Account types & Tauri shell

### Task 8: Account & Provider types

**Files:**
- Create: `crates/usage-core/src/account.rs`
- Modify: `lib.rs` (add `pub mod account;`)

**Interfaces:**
- Produces:
  - `enum Provider { Codex, Claude, Agy }` with `fn as_str(&self)` / `fn from_str(&str) -> Option<Provider>` and `serde` (de)serialize as lowercase strings.
  - `struct Account { pub id: String, pub provider: Provider, pub label: String }` (Serialize/Deserialize).
  - `struct Credentials { pub access_token: String, pub refresh_token: Option<String>, pub account_id: Option<String>, pub expires_at: Option<DateTime<Utc>> }`.

- [ ] **Step 1: Write failing test**

```rust
#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn provider_roundtrips_lowercase() {
        assert_eq!(Provider::from_str("codex"), Some(Provider::Codex));
        assert_eq!(Provider::Agy.as_str(), "agy");
        let j = serde_json::to_string(&Provider::Claude).unwrap();
        assert_eq!(j, "\"claude\"");
    }
}
```

- [ ] **Step 2: Run to verify fail**

Run: `cargo test -p usage-core account`
Expected: FAIL.

- [ ] **Step 3: Implement `account.rs`**

```rust
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Provider { Codex, Claude, Agy }

impl Provider {
    pub fn as_str(&self) -> &'static str {
        match self { Provider::Codex => "codex", Provider::Claude => "claude", Provider::Agy => "agy" }
    }
    pub fn from_str(s: &str) -> Option<Provider> {
        match s { "codex" => Some(Provider::Codex), "claude" => Some(Provider::Claude), "agy" => Some(Provider::Agy), _ => None }
    }
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Account { pub id: String, pub provider: Provider, pub label: String }

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Credentials {
    pub access_token: String,
    pub refresh_token: Option<String>,
    pub account_id: Option<String>,
    pub expires_at: Option<DateTime<Utc>>,
}
```

Add `pub mod account;` to `lib.rs`.

- [ ] **Step 4: Run to verify pass**

Run: `cargo test -p usage-core account`
Expected: PASS.

- [ ] **Step 5: Commit**

```bash
git add crates/usage-core/src/account.rs crates/usage-core/src/lib.rs
git commit -m "feat(core): account and provider types"
```

### Task 9: Investigate agy usage API (spike) + document decision

**Files:**
- Create: `docs/superpowers/notes/agy-usage-investigation.md`

**Interfaces:** none (research spike).

- [ ] **Step 1: Investigate** whether Antigravity/Gemini exposes a quota-% endpoint analogous to Codex/Claude. Check: `agy` CLI config under `~/.gemini`, any `*.json` auth/usage snapshot, and observed network calls. Search the existing repo `Sources/UsageCheckCore/LogScanners.swift` for how Gemini is currently handled.

- [ ] **Step 2: Record the decision** in the notes file: either (a) a real endpoint + auth header shape to add a `fetch/agy.rs` parser in a follow-up, or (b) confirmation that only local-log aggregation is available — in which case agy cards display token totals, not %-gauges.

- [ ] **Step 3: Commit**

```bash
git add docs/superpowers/notes/agy-usage-investigation.md
git commit -m "docs: agy usage-source investigation"
```

### Task 10: Tauri app scaffold (builds & shows a window)

**Files:**
- Create: `src-tauri/Cargo.toml`, `src-tauri/tauri.conf.json`, `src-tauri/build.rs`, `src-tauri/src/main.rs`
- Create: `ui/index.html`, `ui/package.json`, `ui/vite.config.ts`, `ui/src/main.ts`

**Interfaces:**
- Produces: a runnable `usage-app` Tauri binary depending on `usage-core`.

- [ ] **Step 1: Add `src-tauri/Cargo.toml`**

```toml
[package]
name = "usage-app"
version = "0.1.0"
edition = "2021"

[build-dependencies]
tauri-build = { version = "2", features = [] }

[dependencies]
usage-core = { path = "../crates/usage-core" }
tauri = { version = "2", features = ["tray-icon"] }
serde = { version = "1", features = ["derive"] }
serde_json = "1"
tokio = { version = "1", features = ["full"] }
reqwest = { version = "0.12", features = ["json", "rustls-tls"], default-features = false }
keyring = "3"
chrono = { version = "0.4", features = ["serde"] }
tiny_http = "0.12"
open = "5"
```

- [ ] **Step 2: Add `src-tauri/build.rs`**

```rust
fn main() { tauri_build::build() }
```

- [ ] **Step 3: Add minimal `tauri.conf.json`** (frontendDist points to `../ui/dist`, dev URL `http://localhost:5173`, one hidden main window, `app.trayIcon` with an `iconPath`). Follow the current Tauri v2 schema from the tauri docs (use context7/tauri docs for exact keys).

- [ ] **Step 4: Add `src-tauri/src/main.rs`**

```rust
fn main() {
    tauri::Builder::default()
        .run(tauri::generate_context!())
        .expect("error while running tauri application");
}
```

- [ ] **Step 5: Add minimal `ui/` (Vite + TS)** — `index.html` with `<div id="app">UsageCheck</div>`, `package.json` with `vite` and a `build`/`dev` script, `vite.config.ts` default.

- [ ] **Step 6: Verify build**

Run: `cd ui && npm install && npm run build && cd .. && cargo build -p usage-app`
Expected: both succeed.

- [ ] **Step 7: Commit**

```bash
git add src-tauri ui
git commit -m "chore: scaffold Tauri app depending on usage-core"
```

### Task 11: Tray icon + borderless popup toggle (manual smoke)

**Files:**
- Modify: `src-tauri/src/main.rs`
- Create: `src-tauri/icons/tray.png` (a simple monochrome template icon)

**Interfaces:**
- Consumes: Tauri builder.
- Produces: a tray icon that, on left click, shows/hides a borderless popup window anchored near the tray.

- [ ] **Step 1: Implement tray + toggle** using `tauri::tray::TrayIconBuilder` with `on_tray_icon_event` handling `TrayIconEvent::Click`; get the main window via `app.get_webview_window("main")` and toggle `show()`/`hide()`. Set the window `decorations: false`, `skip_taskbar: true` (Windows), `always_on_top: true` in `tauri.conf.json`. Reference the current Tauri v2 tray API via context7/tauri docs for exact signatures.

- [ ] **Step 2: Manual smoke**

Run: `cargo tauri dev` (or `cargo run -p usage-app`)
Expected: tray icon appears in macOS menu bar; clicking toggles a borderless popup. (On Windows this validates taskbar tray + skip-taskbar popup; verify on a Windows machine when available.)

- [ ] **Step 3: Commit**

```bash
git add src-tauri
git commit -m "feat(app): tray icon toggles borderless popup"
```

---

## Phase 5 — Storage, OAuth, polling, UI

### Task 12: Keychain-backed AccountStore

**Files:**
- Create: `src-tauri/src/store.rs`
- Modify: `src-tauri/src/main.rs` (`mod store;`)

**Interfaces:**
- Consumes: `usage_core::account::{Account, Provider, Credentials}`.
- Produces: `struct AccountStore` with:
  - `fn list(&self) -> Vec<Account>` (account index stored as JSON in keychain entry `usagecheck/index`).
  - `fn add(&self, provider: Provider, label: String, creds: Credentials) -> Account`
  - `fn remove(&self, id: &str)`
  - `fn credentials(&self, id: &str) -> Option<Credentials>` (per-account keychain entry `usagecheck/cred/<id>`).

- [ ] **Step 1: Write a test** that round-trips the index (de)serialization helper `serialize_index(&[Account]) -> String` / `parse_index(&str) -> Vec<Account>` (pure functions, so the keychain I/O stays a thin wrapper). Put these pure helpers in `store.rs` and test them.

```rust
#[cfg(test)]
mod tests {
    use super::*;
    use usage_core::account::{Account, Provider};

    #[test]
    fn index_roundtrips() {
        let accts = vec![Account { id: "1".into(), provider: Provider::Codex, label: "work".into() }];
        let s = serialize_index(&accts);
        assert_eq!(parse_index(&s), accts);
    }
}
```

- [ ] **Step 2: Run to verify fail**

Run: `cargo test -p usage-app store`
Expected: FAIL.

- [ ] **Step 3: Implement `store.rs`** — `serialize_index`/`parse_index` via `serde_json`; `AccountStore` methods wrap `keyring::Entry::new("usagecheck", key)` for `get_password`/`set_password`/`delete_credential`. Generate `id` from a UUID-like value (use timestamp + counter, or add the `uuid` crate).

- [ ] **Step 4: Run to verify pass**

Run: `cargo test -p usage-app store`
Expected: PASS (pure helpers). Keychain paths validated in Task 16 smoke.

- [ ] **Step 5: Commit**

```bash
git add src-tauri/src/store.rs src-tauri/src/main.rs
git commit -m "feat(app): keychain-backed account store"
```

### Task 13: OAuth (PKCE) manager with localhost callback

**Files:**
- Create: `src-tauri/src/oauth.rs`
- Modify: `src-tauri/src/main.rs` (`mod oauth;`)

**Interfaces:**
- Consumes: `Provider`, `Credentials`.
- Produces:
  - pure helpers `fn make_pkce() -> (String /*verifier*/, String /*challenge S256*/)` and `fn build_authorize_url(cfg: &ProviderOAuth, challenge: &str, redirect: &str, state: &str) -> String` — both unit-tested.
  - `struct ProviderOAuth { client_id, auth_url, token_url, scopes }` with a `fn config(provider: Provider) -> ProviderOAuth` (client_ids/endpoints filled from Task 9 + provider CLI research; leave `agy` behind the fallback if unavailable).
  - async `fn begin_login(provider) -> Result<Credentials>` — spins a `tiny_http` server on a localhost port, opens the browser via `open`, waits for the `?code=...` callback, exchanges the code at `token_url`, returns `Credentials`.

- [ ] **Step 1: Write failing test** for the pure helpers.

```rust
#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn pkce_challenge_is_url_safe_no_padding() {
        let (verifier, challenge) = make_pkce();
        assert!(verifier.len() >= 43);
        assert!(!challenge.contains('=') && !challenge.contains('+') && !challenge.contains('/'));
    }

    #[test]
    fn authorize_url_contains_params() {
        let cfg = ProviderOAuth {
            client_id: "cid".into(), auth_url: "https://auth.example/authorize".into(),
            token_url: "https://auth.example/token".into(), scopes: "openid".into(),
        };
        let url = build_authorize_url(&cfg, "chal", "http://127.0.0.1:1455/cb", "st8");
        assert!(url.contains("client_id=cid"));
        assert!(url.contains("code_challenge=chal"));
        assert!(url.contains("code_challenge_method=S256"));
        assert!(url.contains("state=st8"));
    }
}
```

- [ ] **Step 2: Run to verify fail**

Run: `cargo test -p usage-app oauth`
Expected: FAIL.

- [ ] **Step 3: Implement `oauth.rs`** — PKCE via `sha2` + base64url (add `sha2`, `base64`, `urlencoding` crates); `build_authorize_url` assembles query params; `begin_login` runs the callback server and token exchange with `reqwest`. Pull each provider's real `client_id`/`auth_url`/`token_url` from the CLI login flow (documented in the Task 9 notes / provider research). If a provider's OAuth cannot be reproduced, mark `config()` to return an error that the UI surfaces as "use fallback import".

- [ ] **Step 4: Run to verify pass**

Run: `cargo test -p usage-app oauth`
Expected: PASS (pure helpers). Full flow validated in Task 16 smoke.

- [ ] **Step 5: Commit**

```bash
git add src-tauri/src/oauth.rs src-tauri/src/main.rs
git commit -m "feat(app): PKCE OAuth manager with localhost callback"
```

### Task 14: Poller — build UsageSnapshot from accounts

**Files:**
- Create: `src-tauri/src/poller.rs`
- Modify: `src-tauri/src/main.rs` (`mod poller;`)

**Interfaces:**
- Consumes: `AccountStore`, `usage_core::fetch::{codex,claude}`, scanners, `Credentials`.
- Produces:
  - `struct AccountUsage { pub account: Account, pub five_hour: Option<QuotaUsage>, pub week: Option<QuotaUsage>, pub totals: WindowTotals, pub status: String }` (Serialize).
  - async `fn poll_all(store: &AccountStore) -> Vec<AccountUsage>` — for each account: refresh token if expiring, call the provider fetcher (Codex/Claude), fall back to local-log aggregation on failure, set `status` ("ok"/"needs_login"/"error"). agy uses log aggregation per Task 9.

- [ ] **Step 1: Write failing test** for the pure mapping `fn account_usage_from_codex(account: &Account, quota: &CodexQuota, totals: WindowTotals) -> AccountUsage`.

```rust
#[cfg(test)]
mod tests {
    use super::*;
    use usage_core::account::{Account, Provider};
    use usage_core::fetch::codex::CodexQuota;
    use usage_core::models::{QuotaUsage, WindowTotals};

    #[test]
    fn maps_codex_quota_to_account_usage() {
        let acct = Account { id: "1".into(), provider: Provider::Codex, label: "w".into() };
        let quota = CodexQuota { plan: None,
            five_hour: Some(QuotaUsage{percent:12.0,resets_at:None,window_seconds:None}), week: None };
        let au = account_usage_from_codex(&acct, &quota, WindowTotals::default());
        assert_eq!(au.status, "ok");
        assert_eq!(au.five_hour.as_ref().unwrap().percent, 12.0);
    }
}
```

- [ ] **Step 2: Run to verify fail**

Run: `cargo test -p usage-app poller`
Expected: FAIL.

- [ ] **Step 3: Implement `poller.rs`** — the pure `account_usage_from_codex` / `account_usage_from_claude` mappers plus the async `poll_all` that performs HTTP (reqwest) with the endpoints from Global Constraints and falls back to scanning `~/.codex/sessions` etc. via `usage_core::scanners` + `aggregate`.

- [ ] **Step 4: Run to verify pass**

Run: `cargo test -p usage-app poller`
Expected: PASS.

- [ ] **Step 5: Commit**

```bash
git add src-tauri/src/poller.rs src-tauri/src/main.rs
git commit -m "feat(app): poller builds usage snapshot per account"
```

### Task 15: Tauri commands + UI wiring

**Files:**
- Modify: `src-tauri/src/main.rs` (register commands + spawn poll loop, emit `usage-updated` event)
- Create: `ui/src/api.ts`, modify `ui/src/main.ts`, `ui/index.html`, add `ui/src/style.css`

**Interfaces:**
- Consumes: `AccountStore`, `poller::poll_all`, `oauth::begin_login`.
- Produces Tauri commands:
  - `#[tauri::command] list_accounts() -> Vec<Account>`
  - `#[tauri::command] add_account(provider: String) -> Result<Account, String>` (runs OAuth)
  - `#[tauri::command] remove_account(id: String)`
  - `#[tauri::command] get_usage() -> Vec<AccountUsage>`
  - a background task that calls `poll_all` every 60s and `app.emit("usage-updated", snapshot)`.

- [ ] **Step 1: Implement commands** in `main.rs`, manage `AccountStore` in Tauri state, register with `.invoke_handler(tauri::generate_handler![...])`, spawn the 60s loop in `.setup(...)`.

- [ ] **Step 2: Implement `ui/src/api.ts`** — thin `invoke` wrappers:

```ts
import { invoke } from "@tauri-apps/api/core";
import { listen } from "@tauri-apps/api/event";
export const listAccounts = () => invoke<Account[]>("list_accounts");
export const addAccount = (provider: string) => invoke<Account>("add_account", { provider });
export const removeAccount = (id: string) => invoke<void>("remove_account", { id });
export const getUsage = () => invoke<AccountUsage[]>("get_usage");
export const onUsage = (cb: (u: AccountUsage[]) => void) => listen<AccountUsage[]>("usage-updated", e => cb(e.payload));
```

- [ ] **Step 3: Implement `ui/src/main.ts`** — render provider-grouped account cards, each with two gauges (5h %, 7d %) + reset time + status badge, and an "계정 추가" button that calls `addAccount` after a provider picker. Subscribe via `onUsage`. Use plain DOM (no framework) for a small popup.

- [ ] **Step 4: Build + manual smoke**

Run: `cd ui && npm run build && cd .. && cargo tauri dev`
Expected: popup lists accounts and live gauges; "계정 추가" launches OAuth; removing works.

- [ ] **Step 5: Commit**

```bash
git add src-tauri ui
git commit -m "feat: Tauri commands and popup UI with account gauges"
```

### Task 16: End-to-end smoke + README + release build

**Files:**
- Modify: `README.md`
- Create: `docs/superpowers/notes/smoke-checklist.md`

- [ ] **Step 1: Manual E2E** (record results in the smoke checklist): add a Codex account via OAuth → gauge shows 5h/7d %; add a Claude account → gauges populate; add an agy account (or fallback import) → token totals/best-effort; remove an account; restart app → accounts persist from keychain; token expiry → auto-refresh or "needs_login" badge.

- [ ] **Step 2: Update `README.md`** — new Tauri build/run instructions (`cargo tauri dev`, `cargo tauri build`), multi-account usage, platform notes (mac menu bar / Windows taskbar), and that the old Swift app is reference-only.

- [ ] **Step 3: Release build check**

Run: `cargo test -p usage-core && cargo build -p usage-app --release`
Expected: all tests pass; release binary builds. (Windows packaging verified on a Windows host when available.)

- [ ] **Step 4: Commit**

```bash
git add README.md docs/superpowers/notes/smoke-checklist.md
git commit -m "docs: E2E smoke checklist and updated README"
```

---

## Self-Review

**Spec coverage:**
- §2 Architecture (usage-core + Tauri shell) → Tasks 0, 10, 11, 15. ✓
- §3 Multi-account & OAuth → Tasks 8, 12 (store), 13 (OAuth), plus fallback surfaced in Task 13/15. ✓
- §4 Data collection (Codex/Claude API + local fallback; agy local) → Tasks 3, 4 (parse), 5, 6, 7 (scanners), 9 (agy spike), 14 (poller wiring fallback). ✓
- §5 Display (per-account 5h + weekly gauges) → Task 15. ✓
- §6 Error handling (status badges, last snapshot) → Task 14 (status), Task 15 (badges). ✓
- §7 Testing (port Swift unit tests to Rust; manual smoke) → Tasks 1–8 unit tests, Tasks 11/16 smoke. ✓
- §8 Risks (OAuth reproduction, agy API) → Task 9 spike + Task 13 fallback path. ✓
- §9 Out of scope (history timeline, TrafficMonitor plugin) → not planned. ✓

**Placeholder scan:** Task 3/4/13 reference "provider research"/context7 for exact Tauri v2 keys and provider client_ids — these are genuine external-lookup steps (endpoints/schema live outside the repo), not vague logic placeholders; the code they feed is fully specified. No "TODO/handle edge cases" logic gaps.

**Type consistency:** `QuotaUsage`, `WindowTotals`, `ModelTokenEvent`, `Account`, `Provider`, `Credentials`, `CodexQuota`, `ClaudeQuota`, `AccountUsage` names are used consistently across tasks; `aggregate(events, now)`, `parse_*` fetchers/scanners, `poll_all`, `account_usage_from_codex` signatures match their consumers.
