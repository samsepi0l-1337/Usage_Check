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

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum LocalProvenance {
    Ok,
    NoEvents,
    NoLocalProfile,
    SharedProfileOther,
    Assumed,
    Ambiguous,
    Conflict,
    Partial,
    Unavailable,
    Truncated,
}

impl LocalProvenance {
    /// Strict total order for deterministic merging. Higher rank = more severe.
    pub fn severity_rank(&self) -> u8 {
        match self {
            LocalProvenance::Truncated => 9,
            LocalProvenance::Unavailable => 8,
            LocalProvenance::Partial => 7,
            LocalProvenance::Conflict => 6,
            LocalProvenance::Ambiguous => 5,
            LocalProvenance::SharedProfileOther => 4,
            LocalProvenance::NoLocalProfile => 3,
            LocalProvenance::Assumed => 2,
            LocalProvenance::NoEvents => 1,
            LocalProvenance::Ok => 0,
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct LocalUsage {
    pub totals: WindowTotals,
    pub provenance: LocalProvenance,
}

impl LocalUsage {
    pub fn none(p: LocalProvenance) -> Self {
        LocalUsage {
            totals: WindowTotals::default(),
            provenance: p,
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum RootIdentity {
    /// Codex auth.json identity (account_id and/or email).
    CodexAuth { account_id: Option<String>, email: Option<String> },
    /// Claude config identity (email).
    ClaudeEmail { email: Option<String> },
    /// No identity evidence available.
    None,
}

impl RootIdentity {
    /// Returns true if both account_id and email are absent (no identity proof).
    pub fn is_absent(&self) -> bool {
        matches!(
            self,
            RootIdentity::CodexAuth { account_id: None, email: None }
                | RootIdentity::ClaudeEmail { email: None }
                | RootIdentity::None
        )
    }
}
