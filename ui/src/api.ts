import { invoke } from "@tauri-apps/api/core";
import { listen } from "@tauri-apps/api/event";

/** Matches `usage_core::account::Provider` (serde lowercase strings). */
export type Provider = "codex" | "claude" | "agy";

/** Matches `usage_core::account::Account`. */
export interface Account {
  id: string;
  provider: Provider;
  label: string;
}

/** Matches `usage_core::models::QuotaUsage`. */
export interface QuotaUsage {
  percent: number;
  resets_at: string | null;
  window_seconds: number | null;
}

/** Matches `usage_core::models::WindowTotals`. */
export interface WindowTotals {
  five_hours: number;
  week: number;
  month: number;
}

/** Matches `poller::AccountUsage`. */
export interface AccountUsage {
  account: Account;
  five_hour: QuotaUsage | null;
  week: QuotaUsage | null;
  totals: WindowTotals;
  status: string;
}

export const listAccounts = () => invoke<Account[]>("list_accounts");
export const addAccount = (provider: string) => invoke<Account>("add_account", { provider });
export const importAccount = (provider: string) =>
  invoke<Account>("import_account", { provider });
export const removeAccount = (id: string) => invoke<void>("remove_account", { id });
export const getUsage = () => invoke<AccountUsage[]>("get_usage");
export const onUsage = (cb: (u: AccountUsage[]) => void) =>
  listen<AccountUsage[]>("usage-updated", (e) => cb(e.payload));
