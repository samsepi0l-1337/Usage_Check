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
