use usage_core::account::{Account, Provider};

use usage_core::AuthMethod;


#[derive(Clone, Copy, Debug)]
pub struct AuthActionSpec {
    pub provider: Provider,
    pub method: AuthMethod,
    pub event_id: &'static str,
    pub label: &'static str,
}

/// The full registry of Add Account actions, independent of license state.
const ALL_AUTH_ACTION_SPECS: &[AuthActionSpec] = &[
        AuthActionSpec {
            provider: Provider::Codex,
            method: AuthMethod::Cli,
            event_id: "add-codex-cli",
            label: "Add Codex (CLI)",
        },
        AuthActionSpec {
            provider: Provider::Codex,
            method: AuthMethod::BrowserOAuth,
            event_id: "add-codex-oauth",
            label: "Login Codex (browser)",
        },
        AuthActionSpec {
            provider: Provider::Claude,
            method: AuthMethod::Cli,
            event_id: "add-claude-cli",
            label: "Add Claude (CLI)",
        },
        AuthActionSpec {
            provider: Provider::Claude,
            method: AuthMethod::BrowserOAuth,
            event_id: "add-claude-oauth",
            label: "Login Claude (browser)",
        },
        AuthActionSpec {
            provider: Provider::Agy,
            method: AuthMethod::BrowserOAuth,
            event_id: "add-agy-oauth",
            label: "Login Antigravity (browser)",
        },
        AuthActionSpec {
            provider: Provider::Cursor,
            method: AuthMethod::LocalDatabase,
            event_id: "add-cursor-local",
            label: "Import Cursor (local, Experimental)",
        },
        AuthActionSpec {
            provider: Provider::Grok,
            method: AuthMethod::ManagementKeyClipboard,
            event_id: "add-grok-clipboard",
            label: "Import xAI API credits (clipboard)",
        },
        AuthActionSpec {
            provider: Provider::Grok,
            method: AuthMethod::ManagementKeyEnvironment,
            event_id: "add-grok-env",
            label: "Import xAI API credits (env vars)",
        },
        AuthActionSpec {
            provider: Provider::Higgsfield,
            method: AuthMethod::Cli,
            event_id: "add-higgsfield-cli",
            label: "Add Higgsfield (CLI)",
        },
    ];

/// Add Account actions available to the user right now: paid-provider specs
/// are omitted unless `is_pro` — the account stays hidden from "Add Account"
/// until a Pro license unlocks it. Takes the license flag explicitly so it is
/// unit-testable without reading global license state.
pub(crate) fn auth_action_specs_with(is_pro: bool) -> Vec<AuthActionSpec> {
    ALL_AUTH_ACTION_SPECS
        .iter()
        .copied()
        .filter(|spec| is_pro || !usage_core::edition::requires_pro(spec.provider))
        .collect()
}

/// Add Account actions available to the user right now, gated on the live
/// license state.
// Retained for callers/tests that want a live snapshot. `build_menu` must use
// `auth_action_specs_with` so its single license sample is shared by every row.
#[allow(dead_code)]
pub fn auth_action_specs() -> Vec<AuthActionSpec> {
    auth_action_specs_with(crate::license::is_pro())
}

/// Resolves an Add Account tray-menu event through the full auth-action
/// registry. This alone is NOT a license gate — the registry is ungated by
/// design (so a stale/crafted event id still resolves to a spec) — callers
/// that DISPATCH the resolved spec must separately re-check entitlement via
/// [`is_dispatch_allowed`] before triggering the side effect.
pub fn spec_for_event(event_id: &str) -> Option<AuthActionSpec> {
    ALL_AUTH_ACTION_SPECS
        .iter()
        .copied()
        .find(|spec| spec.event_id == event_id)
}

/// Whether `spec` may be DISPATCHED (i.e. actually trigger its side effect)
/// right now. A menu rendered while licensed, a stale menu snapshot, or any
/// crafted event id can resolve `spec_for_event` to a paid-provider spec even
/// after the license is gone — this is the re-check at the point of side
/// effect, independent of whatever the menu happened to render. Takes the
/// license flag explicitly (mirrors [`auth_action_specs_with`]) so it is
/// unit-testable without reading global license state.
pub(crate) fn is_dispatch_allowed(spec: &AuthActionSpec, is_pro: bool) -> bool {
    is_pro || !usage_core::edition::requires_pro(spec.provider)
}

/// Whether the "Add Account" entry for `spec` should be CLICKABLE right now.
///
/// Two independent gates, ANDed:
///  * [`is_dispatch_allowed`] — the paid-provider Pro gate (unchanged).
///  * the Free-state per-provider cap.
///
/// `accounts` MUST come from `AccountStore::list()`, never from a poll
/// snapshot: the first menu is built before any poll has run (`main.rs:150`
/// passes an empty slice), so snapshot-derived enablement would show "Add" as
/// clickable on a Free install that already holds an account (D7).
pub(crate) fn is_add_enabled(spec: &AuthActionSpec, is_pro: bool, accounts: &[Account]) -> bool {
    is_dispatch_allowed(spec, is_pro)
        && (is_pro || !usage_core::edition::free_limit_reached(accounts, spec.provider))
}

/// The rendered label for an "Add Account" entry. When the entry is disabled by
/// the Free-state cap, the reason is appended so the user learns WHY without
/// clicking a row that would only fail. A paid-provider spec never reaches this
/// in a disabled state — those are filtered out of `auth_action_specs_with`
/// entirely — so the appended reason is always the cap.
pub(crate) fn add_entry_label(spec: &AuthActionSpec, enabled: bool) -> String {
    if enabled {
        spec.label.to_string()
    } else {
        format!(
            "{} — Pro required ({} account per provider on Free)",
            spec.label,
            usage_core::edition::FREE_ACCOUNTS_PER_PROVIDER,
        )
    }
}
