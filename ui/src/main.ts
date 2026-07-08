import { listAccounts, addAccount, removeAccount, getUsage, onUsage } from "./api";
import type { Account, AccountUsage, QuotaUsage, Provider } from "./api";

const PROVIDER_LABEL: Record<Provider, string> = {
  codex: "Codex",
  claude: "Claude",
  agy: "Antigravity (agy)",
};

const STATUS_LABEL: Record<string, string> = {
  ok: "ok",
  needs_login: "needs login",
  error: "error",
};

let usageById = new Map<string, AccountUsage>();

const app = document.getElementById("app")!;

function el<K extends keyof HTMLElementTagNameMap>(
  tag: K,
  className?: string,
  text?: string,
): HTMLElementTagNameMap[K] {
  const node = document.createElement(tag);
  if (className) node.className = className;
  if (text !== undefined) node.textContent = text;
  return node;
}

function gaugeClass(percent: number): string {
  if (percent >= 90) return "gauge-fill crit";
  if (percent >= 70) return "gauge-fill warn";
  return "gauge-fill";
}

function formatResetTime(resetsAt: string | null): string | null {
  if (!resetsAt) return null;
  const d = new Date(resetsAt);
  if (Number.isNaN(d.getTime())) return null;
  return `resets ${d.toLocaleString(undefined, { month: "short", day: "numeric", hour: "numeric", minute: "2-digit" })}`;
}

function formatTokens(n: number): string {
  if (n >= 1_000_000) return `${(n / 1_000_000).toFixed(1)}M`;
  if (n >= 1_000) return `${(n / 1_000).toFixed(1)}k`;
  return String(n);
}

function renderGaugeRow(label: string, quota: QuotaUsage | null): HTMLElement {
  const row = el("div", "gauge-row");
  row.appendChild(el("span", "gauge-label", label));

  const bar = el("div", "gauge-bar");
  const fill = el("div");
  const pct = quota ? Math.max(0, Math.min(100, quota.percent)) : 0;
  fill.className = gaugeClass(pct);
  fill.style.width = `${pct}%`;
  bar.appendChild(fill);
  row.appendChild(bar);

  row.appendChild(el("span", "gauge-pct", quota ? `${quota.percent.toFixed(0)}%` : "—"));
  return row;
}

function renderTotalsRow(label: string, tokens: number): HTMLElement {
  const row = el("div", "totals-row");
  row.appendChild(el("span", undefined, label));
  const val = el("span", "tval", formatTokens(tokens));
  row.appendChild(val);
  return row;
}

function renderCard(usage: AccountUsage): HTMLElement {
  const { account } = usage;
  const card = el("div", "card");
  card.dataset.id = account.id;

  const top = el("div", "card-top");
  const title = el("div", "card-title");
  title.appendChild(el("span", "card-label", account.label));
  title.appendChild(el("span", "card-provider", PROVIDER_LABEL[account.provider] ?? account.provider));
  top.appendChild(title);

  const actions = el("div", "card-actions");
  const badge = el("span", `badge badge-${usage.status}`, STATUS_LABEL[usage.status] ?? usage.status);
  actions.appendChild(badge);

  const removeBtn = el("button", "remove-btn", "✕");
  removeBtn.title = "Remove account";
  removeBtn.addEventListener("click", async () => {
    await removeAccount(account.id);
    await refresh();
  });
  actions.appendChild(removeBtn);
  top.appendChild(actions);
  card.appendChild(top);

  // Has usable live-quota data (Codex/Claude typically); agy has neither, so
  // it falls through to the token-total display.
  const hasQuota = usage.five_hour !== null || usage.week !== null;

  if (hasQuota) {
    const gauges = el("div", "gauges");
    gauges.appendChild(renderGaugeRow("5h", usage.five_hour));
    gauges.appendChild(renderGaugeRow("7d", usage.week));
    card.appendChild(gauges);

    const resetSrc = usage.five_hour?.resets_at ?? usage.week?.resets_at ?? null;
    const resetText = formatResetTime(resetSrc);
    if (resetText) {
      card.appendChild(el("div", "reset-time", resetText));
    }
  } else {
    // No live quota API (e.g. agy) — show local token totals instead.
    const totals = el("div", "gauges");
    totals.appendChild(renderTotalsRow("5h tokens", usage.totals.five_hours));
    totals.appendChild(renderTotalsRow("7d tokens", usage.totals.week));
    card.appendChild(totals);
  }

  return card;
}

function renderList() {
  const list = document.getElementById("account-list");
  if (!list) return;
  list.innerHTML = "";

  if (usageById.size === 0) {
    list.appendChild(el("div", "empty", "계정을 추가해 사용량을 확인하세요."));
    return;
  }

  // Group by provider, preserving a stable provider order.
  const order: Provider[] = ["codex", "claude", "agy"];
  const grouped = new Map<Provider, AccountUsage[]>();
  for (const usage of usageById.values()) {
    const arr = grouped.get(usage.account.provider) ?? [];
    arr.push(usage);
    grouped.set(usage.account.provider, arr);
  }

  for (const provider of order) {
    const items = grouped.get(provider);
    if (!items || items.length === 0) continue;
    for (const usage of items) {
      list.appendChild(renderCard(usage));
    }
  }
}

function applyUsage(usages: AccountUsage[]) {
  usageById = new Map(usages.map((u) => [u.account.id, u]));
  renderList();
}

async function refresh() {
  try {
    const [accounts, usages] = await Promise.all([listAccounts(), getUsage()]);
    // Ensure accounts with no usage entry yet still show up (e.g. right
    // after add, before the next poll tick populates full usage fields).
    const usageMap = new Map(usages.map((u) => [u.account.id, u]));
    const merged: AccountUsage[] = accounts.map((a: Account) => {
      const existing = usageMap.get(a.id);
      if (existing) return existing;
      return {
        account: a,
        five_hour: null,
        week: null,
        totals: { five_hours: 0, week: 0, month: 0 },
        status: "ok",
      };
    });
    applyUsage(merged);
  } catch (err) {
    console.error("refresh failed", err);
  }
}

function showFallbackMessage(message: string) {
  const banner = document.getElementById("fallback-banner");
  if (!banner) return;
  banner.textContent = message;
  banner.classList.remove("hidden");
  setTimeout(() => banner.classList.add("hidden"), 8000);
}

function openProviderPicker() {
  const overlay = el("div", "overlay");
  overlay.id = "provider-overlay";

  const picker = el("div", "picker");
  picker.appendChild(el("h2", undefined, "계정 추가"));

  const providers: Provider[] = ["codex", "claude", "agy"];
  for (const provider of providers) {
    const btn = el("button", "picker-btn", PROVIDER_LABEL[provider]);
    btn.addEventListener("click", async () => {
      overlay.remove();
      try {
        await addAccount(provider);
        await refresh();
      } catch (err) {
        showFallbackMessage(typeof err === "string" ? err : String(err));
      }
    });
    picker.appendChild(btn);
  }

  const cancel = el("button", "picker-cancel", "취소");
  cancel.addEventListener("click", () => overlay.remove());
  picker.appendChild(cancel);

  overlay.appendChild(picker);
  overlay.addEventListener("click", (e) => {
    if (e.target === overlay) overlay.remove();
  });
  app.appendChild(overlay);
}

function renderShell() {
  app.innerHTML = "";

  const header = el("div", "header");
  header.appendChild(el("h1", undefined, "UsageCheck"));
  const addBtn = el("button", "add-btn", "계정 추가");
  addBtn.addEventListener("click", openProviderPicker);
  header.appendChild(addBtn);
  app.appendChild(header);

  const banner = el("div", "fallback-msg hidden");
  banner.id = "fallback-banner";
  app.appendChild(banner);

  const list = el("div", "list");
  list.id = "account-list";
  app.appendChild(list);
}

async function main() {
  renderShell();
  await refresh();
  await onUsage((usages) => applyUsage(usages));
}

main();
