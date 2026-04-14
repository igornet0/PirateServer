import {
  createProxySession,
  fetchBootstrapHints,
  fetchProxySessionXrayConfig,
  fetchProxySessions,
  revokeProxySession,
} from "../api/client.js";
import type { ProxySessionRow } from "../api/types.js";
import { ApiRequestError } from "../api/types.js";
import { onLocaleChange, t } from "../i18n/index.js";
import type { MessageKey } from "../i18n/translations.js";

const PROXY_SESSION_TOKEN_LS = "pirate.proxySessionTokens.v1";
const PROXY_SUBSCRIPTION_LS = "pirate.proxySubscription.v1";

const proxySessionTokenCache = new Map<string, string>();
let proxySessionTokensLoaded = false;

interface SubscriptionCache {
  subscription_token?: string;
  subscription_url?: string;
  pirate_bootstrap_url?: string;
}

const subscriptionCache = new Map<string, SubscriptionCache>();
let subscriptionCacheLoaded = false;

function loadProxySessionTokens(): void {
  if (proxySessionTokensLoaded) {
    return;
  }
  proxySessionTokensLoaded = true;
  try {
    const raw = localStorage.getItem(PROXY_SESSION_TOKEN_LS);
    if (!raw) {
      return;
    }
    const o = JSON.parse(raw) as Record<string, unknown>;
    for (const [k, v] of Object.entries(o)) {
      if (typeof v === "string" && v.length > 0) {
        proxySessionTokenCache.set(k, v);
      }
    }
  } catch {
    /* ignore */
  }
}

function persistSessionToken(sessionId: string, token: string): void {
  loadProxySessionTokens();
  proxySessionTokenCache.set(sessionId, token);
  try {
    const o: Record<string, string> = {};
    for (const [k, v] of proxySessionTokenCache) {
      o[k] = v;
    }
    localStorage.setItem(PROXY_SESSION_TOKEN_LS, JSON.stringify(o));
  } catch {
    /* ignore */
  }
}

function getSessionToken(sessionId: string): string | undefined {
  loadProxySessionTokens();
  return proxySessionTokenCache.get(sessionId);
}

function loadSubscriptionCache(): void {
  if (subscriptionCacheLoaded) {
    return;
  }
  subscriptionCacheLoaded = true;
  try {
    const raw = localStorage.getItem(PROXY_SUBSCRIPTION_LS);
    if (!raw) {
      return;
    }
    const o = JSON.parse(raw) as Record<string, unknown>;
    for (const [k, v] of Object.entries(o)) {
      if (v && typeof v === "object") {
        subscriptionCache.set(k, v as SubscriptionCache);
      }
    }
  } catch {
    /* ignore */
  }
}

function persistSubscription(sessionId: string, sub: SubscriptionCache): void {
  loadSubscriptionCache();
  subscriptionCache.set(sessionId, sub);
  try {
    const o: Record<string, SubscriptionCache> = {};
    for (const [k, v] of subscriptionCache) {
      o[k] = v;
    }
    localStorage.setItem(PROXY_SUBSCRIPTION_LS, JSON.stringify(o));
  } catch {
    /* ignore */
  }
}

function getSubscription(sessionId: string): SubscriptionCache | undefined {
  loadSubscriptionCache();
  return subscriptionCache.get(sessionId);
}

/** Latest rows from last successful load (for menu actions). */
const lastInboundsRows = new Map<string, ProxySessionRow>();

function utf8ToBase64(s: string): string {
  return btoa(unescape(encodeURIComponent(s)));
}

async function buildExportObject(row: ProxySessionRow): Promise<Record<string, unknown>> {
  let grpc_url: string | null | undefined;
  try {
    const h = await fetchBootstrapHints();
    grpc_url = h.grpc_public_url ?? null;
  } catch {
    grpc_url = undefined;
  }
  const sub = getSubscription(row.session_id);
  const o: Record<string, unknown> = {
    type: "pirate-proxy-session",
    version: 1,
    session_id: row.session_id,
    session_token: getSessionToken(row.session_id) ?? null,
    board_label: row.board_label,
    wire_mode: row.wire_mode,
    wire_config: row.wire_config,
    expires_at_unix_ms: row.expires_at_unix_ms,
    policy: row.policy,
  };
  if (grpc_url) {
    o.grpc_url = grpc_url;
  }
  if (sub?.pirate_bootstrap_url) {
    o.pirate_bootstrap_url = sub.pirate_bootstrap_url;
  }
  return o;
}

async function buildExportJsonPretty(row: ProxySessionRow): Promise<string> {
  return JSON.stringify(await buildExportObject(row), null, 2);
}

async function buildExportJsonCompact(row: ProxySessionRow): Promise<string> {
  return JSON.stringify(await buildExportObject(row));
}

async function buildExportDataUrl(row: ProxySessionRow): Promise<string> {
  return `data:application/json;base64,${utf8ToBase64(await buildExportJsonCompact(row))}`;
}

let inboundsMenuDocListenersBound = false;

function closeAllInboundsMenus(): void {
  document.querySelectorAll(".inbounds-menu-dropdown").forEach((el) => {
    el.setAttribute("hidden", "");
  });
  document.querySelectorAll(".inbounds-menu-trigger").forEach((b) => {
    b.setAttribute("aria-expanded", "false");
  });
}

function positionInboundsMenu(trigger: HTMLElement, menu: HTMLElement): void {
  const r = trigger.getBoundingClientRect();
  const w = 220;
  const left = Math.min(Math.max(8, r.right - w), window.innerWidth - w - 8);
  menu.style.position = "fixed";
  menu.style.top = `${r.bottom + 4}px`;
  menu.style.left = `${left}px`;
  menu.style.width = `${w}px`;
}

function bindInboundsMenuDocumentListeners(): void {
  if (inboundsMenuDocListenersBound) {
    return;
  }
  inboundsMenuDocListenersBound = true;
  document.addEventListener("click", (ev) => {
    const el = ev.target as HTMLElement;
    if (el.closest(".inbounds-menu")) {
      return;
    }
    closeAllInboundsMenus();
  });
  document.addEventListener("keydown", (ev) => {
    if (ev.key === "Escape") {
      closeAllInboundsMenus();
    }
  });
}

async function copyTextToClipboard(text: string): Promise<void> {
  await navigator.clipboard.writeText(text);
}

async function showInboundsQr(row: ProxySessionRow): Promise<void> {
  const dlg = document.getElementById("dialog-inbounds-qr") as HTMLDialogElement | null;
  const img = document.getElementById("inbounds-qr-img") as HTMLImageElement | null;
  const hint = document.getElementById("inbounds-qr-hint") as HTMLElement | null;
  if (!dlg || !img) {
    return;
  }
  hint?.setAttribute("hidden", "");
  hint && (hint.textContent = "");
  img.removeAttribute("src");
  const sub = getSubscription(row.session_id);
  const text = sub?.subscription_url ?? (await buildExportJsonCompact(row));
  try {
    const QRCode = (await import("qrcode")).default;
    img.src = await QRCode.toDataURL(text, {
      width: 280,
      margin: 2,
      errorCorrectionLevel: "M",
    });
  } catch {
    if (hint) {
      hint.textContent = t("inbounds.qrError");
      hint.removeAttribute("hidden");
    }
  }
  dlg.showModal();
}

function bindInboundsQrDialog(): void {
  const dlg = document.getElementById("dialog-inbounds-qr") as HTMLDialogElement | null;
  document.getElementById("inbounds-qr-close")?.addEventListener("click", () => {
    dlg?.close();
  });
  dlg?.addEventListener("click", (ev) => {
    if (ev.target === dlg) {
      dlg.close();
    }
  });
}

async function handleInboundsMenuAction(
  action: string,
  row: ProxySessionRow,
): Promise<void> {
  if (action === "qr") {
    await showInboundsQr(row);
    return;
  }
  if (action === "copy-json") {
    try {
      if (row.ingress_protocol != null) {
        const doc = await fetchProxySessionXrayConfig(row.session_id);
        await copyTextToClipboard(JSON.stringify(doc, null, 2));
        return;
      }
    } catch {
      /* fall through to pirate export */
    }
    try {
      await copyTextToClipboard(await buildExportJsonPretty(row));
    } catch {
      /* ignore */
    }
    return;
  }
  if (action === "copy-url") {
    try {
      const sub = getSubscription(row.session_id);
      if (sub?.subscription_url) {
        await copyTextToClipboard(sub.subscription_url);
        return;
      }
      if (sub?.pirate_bootstrap_url) {
        await copyTextToClipboard(sub.pirate_bootstrap_url);
        return;
      }
      await copyTextToClipboard(await buildExportDataUrl(row));
    } catch {
      /* ignore */
    }
    return;
  }
  if (action === "revoke") {
    await revokeSession(row.session_id);
  }
}

function escapeHtml(s: string): string {
  return s
    .replace(/&/g, "&amp;")
    .replace(/</g, "&lt;")
    .replace(/>/g, "&gt;")
    .replace(/"/g, "&quot;");
}

function numOrNeg1(v: string): number {
  const x = v.trim();
  if (x === "" || x === "-1") {
    return -1;
  }
  const n = Number(x);
  return Number.isFinite(n) ? Math.trunc(n) : -1;
}

function pad2(n: number): string {
  return String(n).padStart(2, "0");
}

/** `datetime-local` value in local timezone (minute precision). */
function toDatetimeLocalValue(d: Date): string {
  return `${d.getFullYear()}-${pad2(d.getMonth() + 1)}-${pad2(d.getDate())}T${pad2(d.getHours())}:${pad2(d.getMinutes())}`;
}

function syncCalendarInputMin(): void {
  const cal = document.getElementById("inbounds-max-dur-calendar") as HTMLInputElement | null;
  if (!cal) {
    return;
  }
  cal.min = toDatetimeLocalValue(new Date());
}

function isCalendarMode(): boolean {
  const calWrap = document.getElementById("inbounds-max-dur-calendar-wrap");
  return calWrap !== null && !calWrap.hasAttribute("hidden");
}

function syncDurationToggleLabel(): void {
  const btn = document.getElementById("inbounds-max-dur-mode-toggle") as HTMLButtonElement | null;
  if (!btn) {
    return;
  }
  const key = isCalendarMode()
    ? "inbounds.durationModeToggleToSeconds"
    : "inbounds.durationModeToggleToCalendar";
  btn.dataset.i18n = key;
  btn.textContent = t(key);
}

function setDurationUiMode(calendar: boolean): void {
  const durWrap = document.getElementById("inbounds-max-dur-duration-wrap");
  const calWrap = document.getElementById("inbounds-max-dur-calendar-wrap");
  const label = document.getElementById("inbounds-max-dur-label") as HTMLLabelElement | null;
  const stack = document.getElementById("inbounds-max-dur-stack") as HTMLElement | null;
  if (!durWrap || !calWrap || !stack) {
    return;
  }
  if (calendar) {
    durWrap.setAttribute("hidden", "");
    calWrap.removeAttribute("hidden");
    stack.dataset.mode = "calendar";
    if (label) {
      label.htmlFor = "inbounds-max-dur-calendar";
    }
    const cal = document.getElementById("inbounds-max-dur-calendar") as HTMLInputElement;
    if (!cal.value) {
      cal.value = toDatetimeLocalValue(new Date(Date.now() + 86400_000));
    }
    syncCalendarInputMin();
  } else {
    calWrap.setAttribute("hidden", "");
    durWrap.removeAttribute("hidden");
    stack.dataset.mode = "duration";
    if (label) {
      label.htmlFor = "inbounds-max-dur";
    }
  }
  syncDurationToggleLabel();
  syncLimitByActiveTimeVisibility();
}

function shouldShowLimitByActiveCheckbox(): boolean {
  if (isCalendarMode()) {
    return false;
  }
  const raw = (document.getElementById("inbounds-max-dur") as HTMLInputElement).value.trim();
  const n = Number(raw);
  if (!Number.isFinite(n)) {
    return false;
  }
  return n > 0;
}

function syncLimitByActiveTimeVisibility(): void {
  const row = document.getElementById("inbounds-limit-by-active-row");
  const cb = document.getElementById("inbounds-limit-by-active-time") as HTMLInputElement | null;
  if (!row || !cb) {
    return;
  }
  if (shouldShowLimitByActiveCheckbox()) {
    row.removeAttribute("hidden");
  } else {
    row.setAttribute("hidden", "");
    cb.checked = false;
  }
}

function applyDurPreset(sec: number): void {
  if (isCalendarMode()) {
    const cal = document.getElementById("inbounds-max-dur-calendar") as HTMLInputElement;
    cal.value = toDatetimeLocalValue(new Date(Date.now() + sec * 1000));
  } else {
    const inp = document.getElementById("inbounds-max-dur") as HTMLInputElement;
    inp.value = String(sec);
  }
}

type MaxDurResult = number | { err: MessageKey };

function resolveMaxSessionDurationSec(): MaxDurResult {
  if (isCalendarMode()) {
    const cal = document.getElementById("inbounds-max-dur-calendar") as HTMLInputElement;
    const raw = cal.value?.trim() ?? "";
    if (!raw) {
      return { err: "inbounds.maxDurCalendarRequired" };
    }
    const ms = new Date(raw).getTime();
    if (!Number.isFinite(ms)) {
      return { err: "inbounds.maxDurCalendarRequired" };
    }
    const sec = Math.ceil((ms - Date.now()) / 1000);
    if (sec <= 0) {
      return { err: "inbounds.maxDurCalendarPast" };
    }
    return sec;
  }
  return numOrNeg1((document.getElementById("inbounds-max-dur") as HTMLInputElement).value);
}

type ConnectionPreset = "simple" | "complex" | "extrem";

function markConnectionPresetButtons(active: ConnectionPreset | null): void {
  document.querySelectorAll<HTMLButtonElement>(".inbounds-connection-preset").forEach((b) => {
    b.classList.toggle("is-active", (b.dataset.preset as ConnectionPreset | undefined) === active);
  });
}

function clientSettingsSnippet(preset: ConnectionPreset): string {
  if (preset === "simple") {
    return JSON.stringify(
      {
        _comment:
          "After Create, paste session_token from the response into boards.YOUR_BOARD. Paths are relative to the directory of settings.json.",
        global: { default_rules: {} },
        boards: {
          YOUR_BOARD: {
            session_token: "<SESSION_TOKEN>",
          },
        },
      },
      null,
      2,
    );
  }
  if (preset === "complex") {
    return JSON.stringify(
      {
        _comment:
          "Ship anti-adw.json next to settings or adjust paths. Enable anti_adw_enabled so built-in lists merge with the JSON block list.",
        global: {
          default_rules: {
            block_json: "default-rules/anti-adw.json",
          },
        },
        boards: {
          YOUR_BOARD: {
            session_token: "<SESSION_TOKEN>",
          },
        },
      },
      null,
      2,
    );
  }
  return JSON.stringify(
    {
      _comment:
        "Three JSON slots: block (ads), pass (RU direct), our (foreign via tunnel). Copy files from server-stack/default-rules/.",
      global: {
        default_rules: {
          block_json: "default-rules/anti-adw.json",
          pass_json: "default-rules/ru-full.json",
          our_json: "default-rules/ru-block-domain.json",
        },
      },
      boards: {
        YOUR_BOARD: {
          session_token: "<SESSION_TOKEN>",
          tls_profile: "modern",
          grpc_keep_alive_interval_secs: 30,
        },
      },
    },
    null,
    2,
  );
}

function applyConnectionPreset(preset: ConnectionPreset): void {
  markConnectionPresetButtons(preset);
  setDurationUiMode(false);
  const setVal = (id: string, v: string): void => {
    const el = document.getElementById(id) as
      | HTMLInputElement
      | HTMLSelectElement
      | HTMLTextAreaElement
      | null;
    if (el) {
      el.value = v;
    }
  };
  const ta = document.getElementById("inbounds-client-snippet") as HTMLTextAreaElement | null;
  if (ta) {
    ta.value = clientSettingsSnippet(preset);
  }
  setVal("inbounds-flow", "");
  setVal("inbounds-max-dur", "-1");
  setVal("inbounds-traffic-total", "-1");
  setVal("inbounds-traffic-in", "-1");
  setVal("inbounds-traffic-out", "-1");
  setVal("inbounds-uuid", "");
  setVal("inbounds-password", "");
  setVal("inbounds-method", "");
  setVal("inbounds-username", "");
  const ingCb = document.getElementById("inbounds-ingress-enable") as HTMLInputElement | null;
  if (ingCb) {
    ingCb.checked = false;
  }
  setVal("inbounds-ingress-port", "");
  setVal("inbounds-ingress-udp", "");
  setVal("inbounds-ingress-config", "");
  setVal("inbounds-ingress-tls", "");
  document.getElementById("inbounds-ingress-fields")?.setAttribute("hidden", "");
  const mode = document.getElementById("inbounds-wire-mode") as HTMLSelectElement | null;
  if (preset === "simple") {
    if (mode) {
      mode.value = "5";
    }
    setVal("inbounds-max-dur", "604800");
    setVal("inbounds-traffic-total", String(5 * 1024 * 1024 * 1024));
  } else if (preset === "complex") {
    if (mode) {
      mode.value = "4";
    }
    setVal("inbounds-method", "aes-256-gcm");
  } else {
    if (mode) {
      mode.value = "1";
    }
    setVal("inbounds-flow", "xtls-rprx-vision");
  }
  syncWireRows();
  syncIngressFields();
  syncLimitByActiveTimeVisibility();
}

function bindInboundsPresets(dialog: HTMLDialogElement): void {
  dialog.addEventListener("click", (ev) => {
    const el = ev.target as HTMLElement;
    const presetBtn = el.closest("button.inbounds-connection-preset") as HTMLButtonElement | null;
    const presetId = presetBtn?.dataset.preset;
    if (presetId === "simple" || presetId === "complex" || presetId === "extrem") {
      applyConnectionPreset(presetId);
      return;
    }
    const durBtn = el.closest("button.inbounds-dur-preset") as HTMLButtonElement | null;
    if (durBtn?.dataset.sec) {
      const sec = Number(durBtn.dataset.sec);
      if (Number.isFinite(sec) && sec > 0) {
        applyDurPreset(sec);
        syncLimitByActiveTimeVisibility();
      }
      return;
    }
    const trafficBtn = el.closest("button.inbounds-traffic-preset") as HTMLButtonElement | null;
    if (trafficBtn?.dataset.inputId != null && trafficBtn.dataset.bytes !== undefined) {
      const inp = document.getElementById(trafficBtn.dataset.inputId) as HTMLInputElement | null;
      if (inp) {
        inp.value = trafficBtn.dataset.bytes;
      }
      return;
    }
    const modeBtn = el.closest("#inbounds-max-dur-mode-toggle") as HTMLButtonElement | null;
    if (modeBtn) {
      setDurationUiMode(!isCalendarMode());
      if (isCalendarMode()) {
        document.getElementById("inbounds-max-dur-calendar")?.focus();
      } else {
        document.getElementById("inbounds-max-dur")?.focus();
      }
    }
  });
}

function protocolLabel(mode: number | null): string {
  if (mode === 1) {
    return "VLESS";
  }
  if (mode === 2) {
    return "Trojan";
  }
  if (mode === 3) {
    return "VMess";
  }
  if (mode === 4) {
    return "Shadowsocks";
  }
  if (mode === 5) {
    return "SOCKS5";
  }
  return "—";
}

function ingressShort(row: ProxySessionRow): string {
  if (row.ingress_protocol == null) {
    return "—";
  }
  const names = ["", "VLESS", "VMess", "Trojan", "SS", "SOCKS", "Hya2"];
  const name = names[row.ingress_protocol] ?? String(row.ingress_protocol);
  const p = row.ingress_listen_port;
  return p != null ? `${name}:${p}` : name;
}

function formatExpires(ms: number): string {
  if (ms === -1) {
    return "∞";
  }
  return new Date(ms).toISOString();
}

function formatClientsTotalOnline(row: ProxySessionRow): string {
  const total = row.proxy_tunnels_total;
  const online = row.proxy_tunnels_online;
  if (total === undefined && online === undefined) {
    return "—";
  }
  const ts = total !== undefined && total !== null ? String(total) : "—";
  const os = online !== undefined && online !== null ? String(online) : "—";
  return `${ts} / ${os}`;
}

function syncWireRows(): void {
  const sel = document.getElementById("inbounds-wire-mode") as HTMLSelectElement | null;
  const rowUuid = document.getElementById("inbounds-row-uuid");
  const rowPw = document.getElementById("inbounds-row-password");
  const rowMethod = document.getElementById("inbounds-row-method");
  const rowUser = document.getElementById("inbounds-row-username");
  const rowFlow = document.getElementById("inbounds-row-flow");
  const flowSel = document.getElementById("inbounds-flow") as HTMLSelectElement | null;
  if (!sel || !rowUuid || !rowPw) {
    return;
  }
  const m = sel.value;
  if (m === "2") {
    rowUuid.setAttribute("hidden", "");
    rowPw.removeAttribute("hidden");
    rowMethod?.setAttribute("hidden", "");
    rowUser?.setAttribute("hidden", "");
  } else if (m === "4") {
    rowUuid.setAttribute("hidden", "");
    rowPw.removeAttribute("hidden");
    rowMethod?.removeAttribute("hidden");
    rowUser?.setAttribute("hidden", "");
  } else if (m === "5") {
    rowUuid.setAttribute("hidden", "");
    rowPw.removeAttribute("hidden");
    rowMethod?.setAttribute("hidden", "");
    rowUser?.removeAttribute("hidden");
  } else {
    rowPw.setAttribute("hidden", "");
    rowUuid.removeAttribute("hidden");
    rowMethod?.setAttribute("hidden", "");
    rowUser?.setAttribute("hidden", "");
  }
  if (m === "1" || m === "3") {
    rowFlow?.removeAttribute("hidden");
  } else {
    rowFlow?.setAttribute("hidden", "");
    if (flowSel) {
      flowSel.value = "";
    }
  }
}

function syncIngressFields(): void {
  const cb = document.getElementById("inbounds-ingress-enable") as HTMLInputElement | null;
  const wrap = document.getElementById("inbounds-ingress-fields");
  if (!cb || !wrap) {
    return;
  }
  if (cb.checked) {
    wrap.removeAttribute("hidden");
  } else {
    wrap.setAttribute("hidden", "");
  }
}

function clearDialogError(): void {
  document.getElementById("inbounds-dialog-error")?.setAttribute("hidden", "");
}

function showDialogError(msg: string): void {
  const el = document.getElementById("inbounds-dialog-error");
  if (el) {
    el.textContent = msg;
    el.removeAttribute("hidden");
  }
}

function resetInboundsForm(): void {
  const setVal = (id: string, v: string): void => {
    const el = document.getElementById(id) as
      | HTMLInputElement
      | HTMLSelectElement
      | HTMLTextAreaElement
      | null;
    if (el) {
      el.value = v;
    }
  };
  setVal("inbounds-label", "");
  const mode = document.getElementById("inbounds-wire-mode") as HTMLSelectElement | null;
  if (mode) {
    mode.value = "1";
  }
  setVal("inbounds-uuid", "");
  setVal("inbounds-password", "");
  setVal("inbounds-flow", "");
  setVal("inbounds-max-dur", "-1");
  const cal = document.getElementById("inbounds-max-dur-calendar") as HTMLInputElement | null;
  if (cal) {
    cal.value = "";
  }
  setDurationUiMode(false);
  syncCalendarInputMin();
  setVal("inbounds-traffic-total", "-1");
  setVal("inbounds-traffic-in", "-1");
  setVal("inbounds-traffic-out", "-1");
  setVal("inbounds-max-devices", "");
  setVal("inbounds-recipient", "");
  const limitActive = document.getElementById("inbounds-limit-by-active-time") as HTMLInputElement | null;
  if (limitActive) {
    limitActive.checked = false;
  }
  syncLimitByActiveTimeVisibility();
  syncWireRows();
  const ingCb = document.getElementById("inbounds-ingress-enable") as HTMLInputElement | null;
  if (ingCb) {
    ingCb.checked = false;
  }
  setVal("inbounds-ingress-port", "");
  setVal("inbounds-ingress-udp", "");
  setVal("inbounds-ingress-config", "");
  setVal("inbounds-ingress-tls", "");
  const ingWrap = document.getElementById("inbounds-ingress-fields");
  ingWrap?.setAttribute("hidden", "");
  setVal("inbounds-method", "");
  setVal("inbounds-username", "");
  const snippet = document.getElementById("inbounds-client-snippet") as HTMLTextAreaElement | null;
  if (snippet) {
    snippet.value = "";
  }
  markConnectionPresetButtons(null);
  const out = document.getElementById("inbounds-create-result");
  if (out) {
    out.textContent = "";
    out.setAttribute("hidden", "");
  }
  clearDialogError();
}

export async function loadInbounds(): Promise<void> {
  const tbody = document.getElementById("inbounds-tbody");
  const err = document.getElementById("inbounds-error");
  if (!tbody) {
    return;
  }
  err?.setAttribute("hidden", "");
  tbody.innerHTML = `<tr><td colspan="8">${escapeHtml(t("inbounds.loading"))}</td></tr>`;
  try {
    const page = await fetchProxySessions({ limit: 100 });
    lastInboundsRows.clear();
    for (const r of page.items) {
      lastInboundsRows.set(r.session_id, r);
    }
    tbody.innerHTML = "";
    for (const row of page.items) {
      tbody.appendChild(renderRow(row));
    }
    if (page.items.length === 0) {
      tbody.innerHTML = `<tr><td colspan="8" class="muted">—</td></tr>`;
    }
  } catch (e) {
    if (err) {
      err.textContent = e instanceof ApiRequestError ? e.message : t("inbounds.error");
      err.removeAttribute("hidden");
    }
    tbody.innerHTML = "";
  }
}

function renderRow(row: ProxySessionRow): HTMLTableRowElement {
  const tr = document.createElement("tr");
  const traffic = `${row.bytes_in} / ${row.bytes_out}`;
  const revoked = row.revoked ? "yes" : "no";
  const sid = escapeHtml(row.session_id);
  const dis = row.revoked ? "disabled" : "";
  tr.innerHTML = `
    <td>${escapeHtml(row.board_label)}</td>
    <td>${escapeHtml(protocolLabel(row.wire_mode))}</td>
    <td>${escapeHtml(ingressShort(row))}</td>
    <td>${escapeHtml(formatClientsTotalOnline(row))}</td>
    <td>${escapeHtml(formatExpires(row.expires_at_unix_ms))}</td>
    <td>${escapeHtml(traffic)}</td>
    <td>${escapeHtml(revoked)}</td>
    <td class="inbounds-menu-cell">
      <div class="inbounds-menu">
        <button type="button" class="inbounds-menu-trigger ghost" aria-expanded="false" aria-haspopup="true" aria-label="${escapeHtml(t("inbounds.menuAria"))}" data-session="${sid}">
          <span aria-hidden="true">⋮</span>
        </button>
        <ul class="inbounds-menu-dropdown" hidden role="menu">
          <li>
            <button type="button" class="inbounds-menu-item ghost" role="menuitem" data-action="qr" data-session="${sid}" ${dis}>
              ${escapeHtml(t("inbounds.menuOpenQr"))}
            </button>
          </li>
          <li>
            <button type="button" class="inbounds-menu-item ghost" role="menuitem" data-action="copy-json" data-session="${sid}" ${dis}>
              ${escapeHtml(t("inbounds.menuCopyJson"))}
            </button>
          </li>
          <li>
            <button type="button" class="inbounds-menu-item ghost" role="menuitem" data-action="copy-url" data-session="${sid}" ${dis}>
              ${escapeHtml(t("inbounds.menuCopyUrl"))}
            </button>
          </li>
          <li>
            <button type="button" class="inbounds-menu-item ghost danger" role="menuitem" data-action="revoke" data-session="${sid}" ${dis}>
              ${escapeHtml(t("inbounds.menuRevoke"))}
            </button>
          </li>
        </ul>
      </div>
    </td>
  `;
  return tr;
}

async function revokeSession(sessionId: string): Promise<void> {
  if (!window.confirm(t("inbounds.revokeConfirm"))) {
    return;
  }
  const err = document.getElementById("inbounds-error");
  try {
    await revokeProxySession(sessionId);
    proxySessionTokenCache.delete(sessionId);
    try {
      const raw = localStorage.getItem(PROXY_SESSION_TOKEN_LS);
      if (raw) {
        const o = JSON.parse(raw) as Record<string, string>;
        delete o[sessionId];
        localStorage.setItem(PROXY_SESSION_TOKEN_LS, JSON.stringify(o));
      }
    } catch {
      /* ignore */
    }
    await loadInbounds();
  } catch (e) {
    if (err) {
      err.textContent = e instanceof ApiRequestError ? e.message : String(e);
      err.removeAttribute("hidden");
    }
  }
}

async function submitCreate(): Promise<void> {
  const out = document.getElementById("inbounds-create-result");
  clearDialogError();
  const label =
    (document.getElementById("inbounds-label") as HTMLInputElement | null)?.value?.trim() ?? "";
  if (!label) {
    showDialogError(`${t("inbounds.boardLabel")} required`);
    return;
  }
  const mode = Number(
    (document.getElementById("inbounds-wire-mode") as HTMLSelectElement).value,
  );
  const uuid = (document.getElementById("inbounds-uuid") as HTMLInputElement).value.trim();
  const password = (document.getElementById("inbounds-password") as HTMLInputElement).value;
  const flow = (document.getElementById("inbounds-flow") as HTMLSelectElement).value.trim();
  const wireConfig: Record<string, unknown> = {};
  if (mode === 2) {
    wireConfig.password = password;
  } else if (mode === 4) {
    const meth = (
      document.getElementById("inbounds-method") as HTMLInputElement
    ).value.trim();
    if (!meth) {
      showDialogError(t("inbounds.cipherRequired"));
      return;
    }
    wireConfig.password = password;
    wireConfig.method = meth;
  } else if (mode === 5) {
    const u = (document.getElementById("inbounds-username") as HTMLInputElement).value.trim();
    if (u) {
      wireConfig.username = u;
    }
    if (password) {
      wireConfig.password = password;
    }
  } else {
    wireConfig.uuid = uuid;
  }
  if (flow) {
    wireConfig.flow = flow;
  }

  let ingress:
    | {
        protocol: number;
        listen_port: number;
        listen_udp_port?: number;
        config: Record<string, unknown>;
        tls?: Record<string, unknown>;
      }
    | undefined = undefined;
  const ingEn = document.getElementById("inbounds-ingress-enable") as HTMLInputElement | null;
  if (ingEn?.checked) {
    const proto = Number(
      (document.getElementById("inbounds-ingress-protocol") as HTMLSelectElement).value,
    );
    const listenPort = Number(
      (document.getElementById("inbounds-ingress-port") as HTMLInputElement).value,
    );
    const udpRaw = (document.getElementById("inbounds-ingress-udp") as HTMLInputElement).value.trim();
    const cfgText = (document.getElementById("inbounds-ingress-config") as HTMLTextAreaElement).value.trim();
    const tlsText = (document.getElementById("inbounds-ingress-tls") as HTMLTextAreaElement).value.trim();
    if (!Number.isFinite(listenPort) || listenPort <= 0 || listenPort > 65535) {
      showDialogError(t("inbounds.ingressPortInvalid"));
      return;
    }
    if (!cfgText) {
      showDialogError(t("inbounds.ingressConfigRequired"));
      return;
    }
    let config: Record<string, unknown>;
    try {
      config = JSON.parse(cfgText) as Record<string, unknown>;
    } catch {
      showDialogError(t("inbounds.ingressConfigJson"));
      return;
    }
    let tls: Record<string, unknown> | undefined;
    if (tlsText) {
      try {
        tls = JSON.parse(tlsText) as Record<string, unknown>;
      } catch {
        showDialogError(t("inbounds.ingressTlsJson"));
        return;
      }
    }
    const udp = udpRaw === "" ? undefined : Number(udpRaw);
    ingress = {
      protocol: proto,
      listen_port: listenPort,
      listen_udp_port: udp != null && udp > 0 ? udp : undefined,
      config,
      tls,
    };
  }

  const maxDurRes = resolveMaxSessionDurationSec();
  if (typeof maxDurRes === "object" && "err" in maxDurRes) {
    showDialogError(t(maxDurRes.err));
    return;
  }

  const policy: Record<string, unknown> = {
    max_session_duration_sec: maxDurRes,
    traffic_total_bytes: numOrNeg1(
      (document.getElementById("inbounds-traffic-total") as HTMLInputElement).value,
    ),
    traffic_bytes_in_limit: numOrNeg1(
      (document.getElementById("inbounds-traffic-in") as HTMLInputElement).value,
    ),
    traffic_bytes_out_limit: numOrNeg1(
      (document.getElementById("inbounds-traffic-out") as HTMLInputElement).value,
    ),
    limit_duration_by_active_time: shouldShowLimitByActiveCheckbox()
      ? (document.getElementById("inbounds-limit-by-active-time") as HTMLInputElement).checked
      : false,
  };

  const maxDevRaw = (
    document.getElementById("inbounds-max-devices") as HTMLInputElement
  ).value.trim();
  if (maxDevRaw !== "") {
    const n = Number(maxDevRaw);
    if (!Number.isFinite(n) || n < -1) {
      showDialogError(t("inbounds.maxDevicesInvalid"));
      return;
    }
    policy.max_concurrent_devices_per_session = n;
  }

  const recipient = (
    document.getElementById("inbounds-recipient") as HTMLInputElement
  ).value.trim();
  try {
    const r = await createProxySession({
      board_label: label,
      policy,
      wire_mode: mode,
      wire_config: wireConfig,
      recipient_client_pubkey_b64: recipient || undefined,
      ingress,
    });
    persistSessionToken(r.session_id, r.session_token);
    if (
      r.subscription_token != null ||
      r.subscription_url != null ||
      r.pirate_bootstrap_url != null
    ) {
      persistSubscription(r.session_id, {
        subscription_token: r.subscription_token ?? undefined,
        subscription_url: r.subscription_url ?? undefined,
        pirate_bootstrap_url: r.pirate_bootstrap_url ?? undefined,
      });
    }
    if (out) {
      out.textContent = `${t("inbounds.createdToken")}\n\n${JSON.stringify(
        {
          session_id: r.session_id,
          session_token: r.session_token,
          expires_at_unix_ms: r.expires_at_unix_ms,
          subscription_token: r.subscription_token ?? null,
          subscription_url: r.subscription_url ?? null,
          pirate_bootstrap_url: r.pirate_bootstrap_url ?? null,
        },
        null,
        2,
      )}`;
      out.removeAttribute("hidden");
    }
    await loadInbounds();
  } catch (e) {
    showDialogError(e instanceof ApiRequestError ? e.message : String(e));
  }
}

let inboundsLocaleHook = false;

function bindInboundsDialog(): void {
  const dialog = document.getElementById("dialog-inbounds-create") as HTMLDialogElement | null;
  if (!dialog) {
    return;
  }

  bindInboundsPresets(dialog);
  if (!inboundsLocaleHook) {
    inboundsLocaleHook = true;
    onLocaleChange(() => {
      syncDurationToggleLabel();
      syncLimitByActiveTimeVisibility();
    });
  }

  document.getElementById("inbounds-max-dur")?.addEventListener("input", () => {
    syncLimitByActiveTimeVisibility();
  });

  document.getElementById("inbounds-add-open")?.addEventListener("click", () => {
    resetInboundsForm();
    dialog.showModal();
    document.getElementById("inbounds-label")?.focus();
  });

  document.getElementById("inbounds-dialog-cancel")?.addEventListener("click", () => {
    dialog.close();
  });

  dialog.addEventListener("click", (ev) => {
    if (ev.target === dialog) {
      dialog.close();
    }
  });

  dialog.addEventListener("close", () => {
    resetInboundsForm();
  });
}

export function bindInboundsTab(): void {
  bindInboundsDialog();
  bindInboundsQrDialog();
  bindInboundsMenuDocumentListeners();
  loadProxySessionTokens();
  loadSubscriptionCache();
  document.getElementById("inbounds-wire-mode")?.addEventListener("change", syncWireRows);
  document.getElementById("inbounds-ingress-enable")?.addEventListener("change", syncIngressFields);
  document.getElementById("inbounds-refresh")?.addEventListener("click", () => {
    void loadInbounds();
  });
  document.getElementById("tab-inbounds")?.addEventListener("click", () => {
    void loadInbounds();
  });
  document.getElementById("inbounds-submit")?.addEventListener("click", () => {
    void submitCreate();
  });
  document.getElementById("inbounds-tbody")?.addEventListener("click", (ev) => {
    const tEl = ev.target as HTMLElement;
    const trigger = tEl.closest("button.inbounds-menu-trigger") as HTMLButtonElement | null;
    if (trigger?.dataset.session) {
      ev.stopPropagation();
      const menu = trigger.nextElementSibling as HTMLElement | null;
      if (!menu?.classList.contains("inbounds-menu-dropdown")) {
        return;
      }
      const wasOpen = !menu.hasAttribute("hidden");
      closeAllInboundsMenus();
      if (!wasOpen) {
        positionInboundsMenu(trigger, menu);
        menu.removeAttribute("hidden");
        trigger.setAttribute("aria-expanded", "true");
      }
      return;
    }
    const item = tEl.closest("button.inbounds-menu-item") as HTMLButtonElement | null;
    const action = item?.dataset.action;
    const sessionId = item?.dataset.session;
    if (!action || !sessionId || item.disabled) {
      return;
    }
    ev.stopPropagation();
    closeAllInboundsMenus();
    const row = lastInboundsRows.get(sessionId);
    if (!row) {
      return;
    }
    void handleInboundsMenuAction(action, row);
  });
  syncWireRows();
}
