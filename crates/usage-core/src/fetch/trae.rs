use serde_json::Value;

use crate::models::QuotaUsage;

const MONTH_SECS: i64 = 30 * 24 * 60 * 60;

/// Parsed Trae `user_current_entitlement_list` quota (used/limit → used %).
#[derive(Clone, Debug, Default, PartialEq)]
pub struct TraeQuota {
    pub email: Option<String>,
    pub plan: Option<String>,
    pub period: Option<QuotaUsage>,
    pub detail_suffix: Option<String>,
}

fn json_f64(v: &Value) -> Option<f64> {
    match v {
        Value::Null => None,
        other => other
            .as_f64()
            .or_else(|| other.as_i64().map(|n| n as f64))
            .or_else(|| other.as_str().and_then(|s| s.trim().parse().ok())),
    }
}

fn nonempty_str(v: &Value) -> Option<String> {
    v.as_str()
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .map(str::to_string)
}

fn first_f64(root: &Value, paths: &[&[&str]]) -> Option<f64> {
    for path in paths {
        let mut current = root;
        for key in *path {
            current = &current[*key];
            if current.is_null() {
                break;
            }
        }
        if let Some(n) = json_f64(current).filter(|n| n.is_finite()) {
            return Some(n);
        }
    }
    None
}

fn first_str(root: &Value, paths: &[&[&str]]) -> Option<String> {
    for path in paths {
        let mut current = root;
        for key in *path {
            current = &current[*key];
            if current.is_null() {
                break;
            }
        }
        if let Some(s) = nonempty_str(current) {
            return Some(s);
        }
    }
    None
}

fn pack_list(root: &Value) -> &[Value] {
    for path in [
        &["user_entitlement_pack_list"][..],
        &["userEntitlementPackList"][..],
        &["entitlement_pack_list"][..],
        &["packs"][..],
        &["data", "user_entitlement_pack_list"][..],
        &["data", "userEntitlementPackList"][..],
        &["result", "user_entitlement_pack_list"][..],
    ] {
        let mut current = root;
        for key in path {
            current = &current[*key];
        }
        if let Some(list) = current.as_array() {
            return list;
        }
    }
    if let Some(list) = root.as_array() {
        return list;
    }
    &[]
}

fn pack_used_limit(pack: &Value) -> Option<(f64, f64)> {
    let used = first_f64(
        pack,
        &[
            &["usage", "premium_model_fast_amount"],
            &["usage", "premiumModelFastAmount"],
            &["usage", "premium_model_fast_request_usage"],
            &["usage", "premiumModelFastRequestUsage"],
            &["usage", "used"],
            &["usage", "used_amount"],
            &["used"],
            &["used_amount"],
            &["premium_model_fast_amount"],
        ],
    );
    let limit = first_f64(
        pack,
        &[
            &[
                "entitlement_base_info",
                "quota",
                "premium_model_fast_request_limit",
            ],
            &[
                "entitlementBaseInfo",
                "quota",
                "premiumModelFastRequestLimit",
            ],
            &[
                "entitlement_base_info",
                "quota",
                "premiumModelFastRequestLimit",
            ],
            &["quota", "premium_model_fast_request_limit"],
            &["quota", "premiumModelFastRequestLimit"],
            &["limit"],
            &["quota", "limit"],
        ],
    )
    .filter(|n| *n > 0.0);
    if let (Some(used), Some(limit)) = (used, limit) {
        return Some((used, limit));
    }
    let remaining = first_f64(
        pack,
        &[
            &["usage", "remaining"],
            &["remaining"],
            &["usage", "left"],
            &["left"],
        ],
    );
    if let (Some(remaining), Some(limit)) = (remaining, limit) {
        return Some(((limit - remaining).max(0.0), limit));
    }
    None
}

fn pack_plan(pack: &Value) -> Option<String> {
    first_str(
        pack,
        &[
            &[
                "entitlement_base_info",
                "product_extra",
                "subscription_extra",
                "plan_name",
            ],
            &["entitlement_base_info", "plan_name"],
            &["entitlement_base_info", "planName"],
            &["plan_name"],
            &["planName"],
            &["plan"],
        ],
    )
    .or_else(|| {
        let product_id = first_f64(
            pack,
            &[
                &["entitlement_base_info", "product_id"],
                &["entitlementBaseInfo", "productId"],
                &["product_id"],
            ],
        )?;
        Some(if product_id == 0.0 {
            "Free".to_string()
        } else {
            "Pro".to_string()
        })
    })
}

fn pack_end_time(pack: &Value) -> Option<i64> {
    first_f64(
        pack,
        &[
            &["entitlement_base_info", "end_time"],
            &["entitlementBaseInfo", "endTime"],
            &["end_time"],
            &["expire_time"],
            &["expireTime"],
        ],
    )
    .map(|n| n as i64)
    .filter(|n| *n > 0)
}

/// Sums fast-request used/limit across entitlement packs. Remaining-only
/// packs never become a percent.
pub fn parse_trae_entitlements(root: &Value) -> TraeQuota {
    let email = first_str(
        root,
        &[&["email"], &["user", "email"], &["account", "email"]],
    );
    let mut used_total = 0.0;
    let mut limit_total = 0.0;
    let mut plan = None;
    let mut end_time = None;
    for pack in pack_list(root) {
        if plan.is_none() {
            plan = pack_plan(pack);
        }
        if end_time.is_none() {
            end_time = pack_end_time(pack);
        }
        if let Some((used, limit)) = pack_used_limit(pack) {
            used_total += used;
            limit_total += limit;
        }
    }
    let period = if limit_total > 0.0 {
        Some(QuotaUsage {
            percent: ((used_total / limit_total) * 100.0).clamp(0.0, 100.0),
            resets_at: None,
            window_seconds: Some(MONTH_SECS),
        })
    } else {
        None
    };
    let _ = end_time;
    TraeQuota {
        email,
        plan,
        period,
        detail_suffix: None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn sums_fast_request_used_and_limit() {
        let v = json!({
            "user_entitlement_pack_list": [{
                "entitlement_base_info": {
                    "product_id": 1,
                    "quota": { "premium_model_fast_request_limit": 500 }
                },
                "usage": { "premium_model_fast_amount": 125 }
            }]
        });
        let q = parse_trae_entitlements(&v);
        assert_eq!(q.plan.as_deref(), Some("Pro"));
        assert!((q.period.as_ref().unwrap().percent - 25.0).abs() < 0.001);
        assert_eq!(q.period.as_ref().unwrap().window_seconds, Some(MONTH_SECS));
    }

    #[test]
    fn sums_plan_and_extra_packs() {
        let v = json!({
            "userEntitlementPackList": [
                {
                    "entitlementBaseInfo": {
                        "productId": 0,
                        "quota": { "premiumModelFastRequestLimit": 10 }
                    },
                    "usage": { "premiumModelFastAmount": 4 }
                },
                {
                    "entitlement_base_info": {
                        "product_type": 2,
                        "quota": { "premium_model_fast_request_limit": 20 }
                    },
                    "usage": { "premium_model_fast_amount": 6 }
                }
            ]
        });
        let q = parse_trae_entitlements(&v);
        assert!((q.period.as_ref().unwrap().percent - 10.0 / 30.0 * 100.0).abs() < 0.01);
        assert_eq!(q.plan.as_deref(), Some("Free"));
    }

    #[test]
    fn remaining_and_limit_become_used() {
        let v = json!({
            "packs": [{
                "limit": 100,
                "remaining": 40
            }]
        });
        let q = parse_trae_entitlements(&v);
        assert!((q.period.as_ref().unwrap().percent - 60.0).abs() < 0.001);
    }

    #[test]
    fn remaining_only_does_not_invent_percent() {
        let q = parse_trae_entitlements(&json!({
            "user_entitlement_pack_list": [{ "usage": { "remaining": 5 } }]
        }));
        assert!(q.period.is_none());
    }

    #[test]
    fn empty_json_has_no_quota() {
        let q = parse_trae_entitlements(&json!({ "error": "missing" }));
        assert!(q.period.is_none());
        assert!(q.plan.is_none());
    }

    #[test]
    fn zero_used_with_limit_is_zero_percent() {
        let v = json!({
            "user_entitlement_pack_list": [{
                "quota": { "premium_model_fast_request_limit": 50 },
                "usage": { "premium_model_fast_amount": 0 }
            }]
        });
        let q = parse_trae_entitlements(&v);
        assert_eq!(q.period.as_ref().unwrap().percent, 0.0);
    }
}
