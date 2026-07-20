//! CSV export of the current usage snapshot, served at `GET /v1/usage.csv`.
//!
//! A flat one-row-per-(account, window) shape convenient for spreadsheets and
//! quick `curl | column -s, -t` inspection. Kept in a sibling module so
//! `api.rs` stays within the file-size budget and rendering is unit-testable.

use crate::api::UsageResponse;

/// Content-Type for the CSV export.
pub const CSV_CONTENT_TYPE: &str = "text/csv; charset=utf-8";

/// Header row; column order matches [`push_row`].
const CSV_HEADER: &str = "provider,account,plan,status,window,pool,used_percent\n";

/// Field escaping for CSV output:
/// 1. Formula-injection guard: a leading `=`, `+`, `-`, `@` (or tab/CR) makes
///    Excel/Sheets treat the cell as a formula, so prefix a `'` to force text.
/// 2. RFC-4180 quoting: wrap in double-quotes and double any interior quote
///    when the value contains a comma, quote, or newline.
fn csv_field(v: &str) -> String {
    let guarded = if v
        .as_bytes()
        .first()
        .is_some_and(|b| matches!(b, b'=' | b'+' | b'-' | b'@' | b'\t' | b'\r'))
    {
        format!("'{v}")
    } else {
        v.to_string()
    };
    if guarded.contains([',', '"', '\n', '\r']) {
        format!("\"{}\"", guarded.replace('"', "\"\""))
    } else {
        guarded
    }
}

/// Appends one data row. Non-finite percents (NaN / ±inf) are skipped so the
/// numeric column always parses.
#[allow(clippy::too_many_arguments)]
fn push_row(
    out: &mut String,
    provider: &str,
    account: &str,
    plan: &str,
    status: &str,
    window: &str,
    pool: &str,
    percent: f64,
) {
    if !percent.is_finite() {
        return;
    }
    out.push_str(&format!(
        "{},{},{},{},{},{},{}\n",
        csv_field(provider),
        csv_field(account),
        csv_field(plan),
        csv_field(status),
        window,
        csv_field(pool),
        percent,
    ));
}

/// Renders the usage snapshot as CSV: a header row plus one row per
/// account/window (and per agy pool/window).
pub fn csv_body(resp: &UsageResponse) -> String {
    let mut out = String::from(CSV_HEADER);
    for a in &resp.accounts {
        let provider = a.provider.as_str();
        let plan = a.plan.as_deref().unwrap_or("");
        let account = a.display_name.as_str();
        let status = a.status.as_str();
        if let Some(q) = &a.five_hour {
            push_row(&mut out, provider, account, plan, status, "5h", "", q.used_percent);
        }
        if let Some(q) = &a.week {
            push_row(&mut out, provider, account, plan, status, "7d", "", q.used_percent);
        }
        for pool in &a.pools {
            if let Some(q) = &pool.five_hour {
                push_row(&mut out, provider, account, plan, status, "5h", &pool.name, q.used_percent);
            }
            if let Some(q) = &pool.week {
                push_row(&mut out, provider, account, plan, status, "7d", &pool.name, q.used_percent);
            }
        }
    }
    out
}

#[cfg(test)]
#[path = "api_csv_tests.rs"]
mod tests;
