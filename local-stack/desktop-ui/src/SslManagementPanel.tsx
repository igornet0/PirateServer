/**
 * gRPC Ssl* management for the active server bookmark (identity must match this gRPC URL).
 */
import { invoke } from "@tauri-apps/api/core";
import { AlertCircle, CheckCircle, Loader2, RefreshCw, Shield, XCircle } from "lucide-react";
import React, { useCallback, useEffect, useState } from "react";
import { toast } from "sonner";
import { CopyablePre } from "./ui/CopyablePre";
import { ModalDialog } from "./ui/ModalDialog";

export type SslCertRow = {
  primary_domain: string;
  domains: string[];
  expiry_unix_ms: number;
  status: number;
  live_path: string;
  last_error: string;
  updated_at_ms: number;
  cert_name: string;
};

function tr(ru: string, en: string, lang: "ru" | "en") {
  return lang === "ru" ? ru : en;
}

function statusLabel(
  s: number,
  lang: "ru" | "en",
): { text: string; className: string; Icon: typeof CheckCircle } {
  switch (s) {
    case 1:
      return {
        text: tr("Действителен", "Valid", lang),
        className: "text-emerald-300",
        Icon: CheckCircle,
      };
    case 2:
      return {
        text: tr("Скоро истекает", "Expiring soon", lang),
        className: "text-amber-300",
        Icon: AlertCircle,
      };
    case 3:
      return {
        text: tr("Истёк", "Expired", lang),
        className: "text-rose-300",
        Icon: XCircle,
      };
    case 4:
      return {
        text: tr("Ошибка", "Error", lang),
        className: "text-rose-300",
        Icon: XCircle,
      };
    default:
      return {
        text: tr("Не задан", "Unspecified", lang),
        className: "text-slate-400",
        Icon: AlertCircle,
      };
  }
}

type SslPostCheckJson = {
  nginx_test_ok?: boolean;
  reload_ok?: boolean;
  tls_handshake_ok?: boolean;
  hostname_match_ok?: boolean;
  chain_ok?: boolean;
  upstream_health_ok?: boolean;
  rollback_performed?: boolean;
  summary?: string;
  http_status?: number;
  classified_error?: string;
  probe_host?: string;
  curl_exit?: number;
  details?: { step?: string; ok?: boolean; message?: string }[];
};

function PostCheckBanner({ pc, language }: { pc: SslPostCheckJson; language: "ru" | "en" }) {
  const ok =
    pc.reload_ok !== false &&
    pc.nginx_test_ok !== false &&
    (pc.upstream_health_ok !== false || pc.classified_error === "curl_unavailable");
  return (
    <div
      className={`rounded-xl border px-3 py-2 text-xs ${
        ok ? "border-emerald-800/50 bg-emerald-950/20 text-emerald-100" : "border-amber-800/50 bg-amber-950/30 text-amber-100"
      }`}
    >
      <div className="font-semibold">
        {tr("Проверка после SSL (nginx / HTTPS)", "Post-check (nginx / HTTPS)", language)}
      </div>
      {pc.summary ? <p className="mt-1 text-slate-200/90">{pc.summary}</p> : null}
      {pc.classified_error === "tls_name_mismatch" ? (
        <p className="mt-1 text-amber-200/90">
          {tr(
            "Имя хоста проверки не совпадает с CN/SAN сертификата или nginx отдаёт другой сертификат. См. docs/SSL_CURL_60_RUNBOOK.md.",
            "Probe hostname does not match certificate CN/SAN, or nginx serves a different certificate. See docs/SSL_CURL_60_RUNBOOK.md.",
            language,
          )}
        </p>
      ) : null}
      <ul className="mt-2 grid grid-cols-2 gap-x-4 gap-y-1 text-[11px] text-slate-400 sm:grid-cols-3">
        <li>
          nginx -t: {pc.nginx_test_ok === false ? "✗" : "✓"}
        </li>
        <li>
          reload: {pc.reload_ok === false ? "✗" : "✓"}
        </li>
        <li>
          HTTPS: {pc.upstream_health_ok === false && pc.classified_error !== "curl_unavailable" ? "✗" : "✓"}
        </li>
        {pc.http_status ? (
          <li>
            HTTP {tr("код", "code", language)}: {pc.http_status}
          </li>
        ) : null}
        {pc.classified_error ? (
          <li className="col-span-2 sm:col-span-3">
            {tr("Класс", "Class", language)}: {pc.classified_error}
          </li>
        ) : null}
        {pc.probe_host ? (
          <li className="col-span-2 sm:col-span-3">
            SNI / probe host: <code className="text-slate-300">{pc.probe_host}</code>
          </li>
        ) : null}
        {pc.curl_exit != null && pc.curl_exit !== 0 ? (
          <li className="col-span-2 sm:col-span-3">
            curl {tr("код выхода", "exit", language)}: {pc.curl_exit}
          </li>
        ) : null}
      </ul>
    </div>
  );
}

function formatExpiry(ms: number, lang: "ru" | "en") {
  if (!ms) return "—";
  try {
    return new Date(ms).toLocaleString(lang === "ru" ? "ru-RU" : "en-GB", {
      dateStyle: "medium",
      timeStyle: "short",
    });
  } catch {
    return "—";
  }
}

const SSL_MODE_OPTIONS: { v: number; ru: string; en: string }[] = [
  { v: 0, ru: "Как в окружении (SSL_MODE)", en: "From env (SSL_MODE)" },
  { v: 1, ru: "nginx", en: "nginx" },
  { v: 2, ru: "standalone", en: "standalone" },
  { v: 3, ru: "webroot", en: "webroot" },
  { v: 4, ru: "DNS-01 (certbot + env)", en: "DNS-01 (certbot + env)" },
];

function parseDomainList(input: string): string[] {
  const parts = input
    .split(/[\s,;]+/u)
    .map((s) => s.trim().toLowerCase())
    .filter(Boolean);
  return Array.from(new Set(parts));
}

function readEnvKey(content: string, key: string): string {
  const lines = content.split(/\r?\n/);
  const prefix = `${key}=`;
  for (const line of lines) {
    if (line.startsWith(prefix) && !line.trimStart().startsWith("#")) {
      return line.slice(prefix.length).trim();
    }
  }
  return "";
}

function upsertEnvKey(content: string, key: string, value: string): string {
  const esc = key.replace(/[.*+?^${}()|[\]\\]/g, "\\$&");
  const re = new RegExp(`^\\s*${esc}=`, "m");
  const lines = content.split(/\r?\n/);
  let found = false;
  const next = lines.map((line) => {
    if (re.test(line) && !line.trimStart().startsWith("#")) {
      found = true;
      return `${key}=${value}`;
    }
    return line;
  });
  if (!found) {
    const tail = content === "" || content.endsWith("\n") ? "" : "\n";
    return `${content}${tail}${key}=${value}\n`;
  }
  return next.join("\n");
}

const btnBase =
  "inline-flex items-center justify-center gap-2 rounded-xl px-4 py-2.5 text-sm font-semibold transition-all duration-200 focus-visible:outline-none focus-visible:ring-2 focus-visible:ring-red-600/80 focus-visible:ring-offset-2 focus-visible:ring-offset-[#050204] active:scale-[0.98] disabled:pointer-events-none disabled:opacity-50";

type Props = {
  grpcUrl: string;
  projectId: string;
  controlBase: string;
  sessionOk: boolean;
  sameServerAsActive: boolean;
  language: "ru" | "en";
  onHostRestartHint?: (msg: string | null) => void;
  onRestartPending?: () => void | Promise<void>;
};

export function SslManagementPanel({
  grpcUrl,
  projectId,
  controlBase,
  sessionOk,
  sameServerAsActive,
  language,
  onHostRestartHint,
  onRestartPending,
}: Props) {
  const [certs, setCerts] = useState<SslCertRow[]>([]);
  const [thresholdDays, setThresholdDays] = useState<number | null>(null);
  const [busy, setBusy] = useState(false);
  const [err, setErr] = useState<string | null>(null);
  const [addOpen, setAddOpen] = useState(false);
  const [detail, setDetail] = useState<SslCertRow | null>(null);
  const [updateTarget, setUpdateTarget] = useState<SslCertRow | null>(null);
  const [logLines, setLogLines] = useState<string[] | null>(null);
  const [postCheck, setPostCheck] = useState<SslPostCheckJson | null>(null);

  const [addDomains, setAddDomains] = useState("");
  const [addMode, setAddMode] = useState(0);
  const [addWebroot, setAddWebroot] = useState("/var/www/html");
  const [addDry, setAddDry] = useState(true);
  const [addStaging, setAddStaging] = useState(true);

  const [updDry, setUpdDry] = useState(true);

  const [renewForce, setRenewForce] = useState(false);
  const [schedBusy, setSchedBusy] = useState(false);
  const [schedInterval, setSchedInterval] = useState("86400");
  const [schedEnable, setSchedEnable] = useState("0");
  const [schedEnvDraft, setSchedEnvDraft] = useState("");

  const load = useCallback(async () => {
    if (!sessionOk || !sameServerAsActive) return;
    setBusy(true);
    setErr(null);
    try {
      const raw = await invoke<string>("ssl_status_json", {
        grpcUrl: grpcUrl.trim(),
        projectId: projectId.trim() || "default",
      });
      const j = JSON.parse(raw) as { certs?: SslCertRow[]; threshold_days?: number };
      setCerts(Array.isArray(j.certs) ? j.certs : []);
      setThresholdDays(typeof j.threshold_days === "number" ? j.threshold_days : null);
    } catch (e) {
      const msg = String(e);
      setErr(msg);
      toast.error(msg);
    } finally {
      setBusy(false);
    }
  }, [sessionOk, sameServerAsActive, grpcUrl, projectId]);

  const loadSchedulerEnv = useCallback(async () => {
    if (!sessionOk) return;
    setSchedBusy(true);
    setErr(null);
    try {
      await invoke("set_control_api_base", { url: controlBase.trim() });
      const raw = await invoke<string>("control_api_fetch_host_deploy_env_json");
      const parsed = JSON.parse(raw) as { content?: string };
      const c = typeof parsed.content === "string" ? parsed.content : "";
      setSchedEnvDraft(c);
      setSchedInterval(readEnvKey(c, "SSL_CHECK_INTERVAL") || "86400");
      setSchedEnable(readEnvKey(c, "SSL_ENABLE_SCHEDULER") || "0");
    } catch (e) {
      setErr(String(e));
    } finally {
      setSchedBusy(false);
    }
  }, [sessionOk, controlBase]);

  useEffect(() => {
    void load();
  }, [load]);

  useEffect(() => {
    if (sessionOk && sameServerAsActive) void loadSchedulerEnv();
  }, [sessionOk, sameServerAsActive, loadSchedulerEnv]);

  const saveSchedulerEnv = async () => {
    setSchedBusy(true);
    setErr(null);
    onHostRestartHint?.(null);
    try {
      await invoke("set_control_api_base", { url: controlBase.trim() });
      let c = schedEnvDraft;
      c = upsertEnvKey(c, "SSL_CHECK_INTERVAL", schedInterval.trim() || "86400");
      c = upsertEnvKey(c, "SSL_ENABLE_SCHEDULER", schedEnable.trim() || "0");
      const raw = await invoke<string>("control_api_put_host_deploy_env", { content: c });
      setSchedEnvDraft(c);
      let scheduled = false;
      try {
        const j = JSON.parse(raw) as { restart_scheduled?: boolean };
        scheduled = Boolean(j.restart_scheduled);
      } catch {
        scheduled = raw.includes("restart_scheduled") && raw.includes("true");
      }
      onHostRestartHint?.(
        scheduled
          ? tr(
              "Запланирован перезапуск сервисов после смены host env.",
              "Services restart scheduled after host env change.",
              language,
            )
          : tr(
              "Host env записан. При отсутствии systemd перезапустите вручную.",
              "Host env saved. Restart manually if needed.",
              language,
            ),
      );
      if (scheduled) onRestartPending?.();
      toast.success(tr("Параметры SSL scheduler сохранены", "SSL scheduler settings saved", language));
    } catch (e) {
      const msg = String(e);
      setErr(msg);
      toast.error(msg);
    } finally {
      setSchedBusy(false);
    }
  };

  const submitAdd = async () => {
    const domains = parseDomainList(addDomains);
    if (domains.length === 0) {
      toast.error(tr("Укажите хотя бы один домен", "Add at least one domain", language));
      return;
    }
    if (!addDry && !addStaging) {
      const ok = window.confirm(
        tr(
          "Выпустить production-сертификат (не dry-run, не staging)?",
          "Issue a production certificate (not dry-run, not staging)?",
          language,
        ),
      );
      if (!ok) return;
    }
    setBusy(true);
    setErr(null);
    setLogLines(null);
    try {
      const raw = await invoke<string>("ssl_create", {
        grpcUrl: grpcUrl.trim(),
        projectId: projectId.trim() || "default",
        domains,
        mode: addMode,
        webrootPath: addWebroot.trim(),
        dryRun: addDry,
        staging: addStaging,
      });
      const j = JSON.parse(raw) as {
        log_lines?: string[];
        status?: string;
        post_check?: SslPostCheckJson;
      };
      setLogLines(Array.isArray(j.log_lines) ? j.log_lines : null);
      const pc = j.post_check;
      setPostCheck(pc && typeof pc === "object" ? pc : null);
      if (j.status === "degraded") {
        toast.warning(
          pc?.summary ||
            tr(
              "Сертификат получен, но проверка nginx/HTTPS не прошла полностью (см. блок ниже).",
              "Certificate issued, but nginx/HTTPS post-check is not fully ok (see panel below).",
              language,
            ),
        );
      } else {
        toast.success(j.status || tr("SslCreate готов", "SslCreate done", language));
      }
      setAddOpen(false);
      await load();
    } catch (e) {
      const msg = String(e);
      setErr(msg);
      toast.error(msg);
    } finally {
      setBusy(false);
    }
  };

  const submitUpdate = async () => {
    if (!updateTarget) return;
    setBusy(true);
    setErr(null);
    setLogLines(null);
    try {
      const raw = await invoke<string>("ssl_update", {
        grpcUrl: grpcUrl.trim(),
        projectId: projectId.trim() || "default",
        exactDomain: updateTarget.primary_domain,
        globPattern: "",
        regex: "",
        dryRun: updDry,
      });
      const j = JSON.parse(raw) as {
        log_lines?: string[];
        status?: string;
        post_check?: SslPostCheckJson;
      };
      setLogLines(Array.isArray(j.log_lines) ? j.log_lines : null);
      const pc = j.post_check;
      setPostCheck(pc && typeof pc === "object" ? pc : null);
      if (j.status === "degraded") {
        toast.warning(pc?.summary || "SslUpdate degraded");
      } else {
        toast.success(j.status || "SslUpdate");
      }
      setUpdateTarget(null);
      await load();
    } catch (e) {
      const msg = String(e);
      setErr(msg);
      toast.error(msg);
    } finally {
      setBusy(false);
    }
  };

  const runRenew = async () => {
    if (renewForce) {
      const ok = window.confirm(
        tr("Принудительно проверить и renew все известные сертификаты?", "Force check and renew all known certificates?", language),
      );
      if (!ok) return;
    }
    setBusy(true);
    setErr(null);
    setLogLines(null);
    try {
      const raw = await invoke<string>("ssl_check_and_renew", {
        grpcUrl: grpcUrl.trim(),
        projectId: projectId.trim() || "default",
        forceAll: renewForce,
      });
      const j = JSON.parse(raw) as {
        log_lines?: string[];
        status?: string;
        post_check?: SslPostCheckJson;
      };
      setLogLines(Array.isArray(j.log_lines) ? j.log_lines : null);
      const pc = j.post_check;
      setPostCheck(pc && typeof pc === "object" ? pc : null);
      if (j.status === "degraded") {
        toast.warning(pc?.summary || tr("Проверка с предупреждением", "Check completed with warnings", language));
      } else {
        toast.success(j.status || tr("Проверка завершена", "Check completed", language));
      }
      await load();
    } catch (e) {
      const msg = String(e);
      setErr(msg);
      toast.error(msg);
    } finally {
      setBusy(false);
    }
  };

  if (!sessionOk) {
    return <p className="text-sm text-slate-500">—</p>;
  }

  if (!sameServerAsActive) {
    return (
      <p className="text-sm text-amber-200/90">
        {tr(
          "Вкладка SSL доступна только для сервера, совпадающего с активным gRPC-подключением. Активируйте эту закладку в «Saved servers».",
          "SSL is only available when this bookmark matches the active gRPC connection. Activate this bookmark in «Saved servers».",
          language,
        )}
      </p>
    );
  }

  return (
    <div className="space-y-4">
      {err ? (
        <p className="flex items-start gap-2 text-sm text-rose-300">
          <AlertCircle className="mt-0.5 h-4 w-4 shrink-0" />
          {err}
        </p>
      ) : null}

      {postCheck ? <PostCheckBanner pc={postCheck} language={language} /> : null}

      <div className="flex flex-wrap items-center gap-2">
        <button
          type="button"
          disabled={busy}
          onClick={() => void load()}
          className={`${btnBase} border border-white/15 bg-white/5 text-slate-200 hover:bg-white/10`}
        >
          {busy ? <Loader2 className="h-4 w-4 animate-spin" /> : <RefreshCw className="h-4 w-4" />}
          {tr("Обновить статус", "Refresh status", language)}
        </button>
        <button
          type="button"
          disabled={busy}
          onClick={() => setAddOpen(true)}
          className={`${btnBase} border border-red-800/40 bg-amber-950/30 text-amber-100 hover:bg-amber-950/50`}
        >
          <Shield className="h-4 w-4" />
          {tr("Создать / обновить (certbot)", "Create / issue (certbot)", language)}
        </button>
        <button
          type="button"
          disabled={busy}
          onClick={() => void runRenew()}
          className={`${btnBase} bg-gradient-to-r from-red-700 to-red-900 text-white shadow-lg shadow-red-950/40 hover:brightness-110`}
        >
          {busy ? <Loader2 className="h-4 w-4 animate-spin" /> : null}
          {tr("Проверить и renew", "Check and renew", language)}
        </button>
        <label className="flex cursor-pointer items-center gap-2 text-xs text-slate-400">
          <input
            type="checkbox"
            checked={renewForce}
            onChange={(e) => setRenewForce(e.target.checked)}
            className="rounded border-white/20 bg-black/40"
          />
          {tr("force_all (все)", "force_all (all)", language)}
        </label>
      </div>

      {thresholdDays != null ? (
        <p className="text-xs text-slate-500">
          {tr("Порог «скоро истекает»", "“Expiring soon” threshold", language)}:{" "}
          <span className="text-slate-300">{thresholdDays}</span> {tr("дн.", "days", language)}
        </p>
      ) : null}

      <p className="text-xs text-slate-500">
        {tr(
          "Провайдер: Certbot / Let’s Encrypt. Email для ACME настраивается на сервере (например SSL_EMAIL в host deploy env), не в этом окне.",
          "Provider: Certbot / Let’s Encrypt. ACME contact email is configured on the server (e.g. SSL_EMAIL in host deploy env), not in this dialog.",
          language,
        )}
      </p>

      <div className="overflow-x-auto rounded-xl border border-white/10">
        <table className="w-full min-w-[32rem] text-left text-xs text-slate-200">
          <thead className="border-b border-white/10 bg-black/30 text-slate-400">
            <tr>
              <th className="px-3 py-2 font-medium">{tr("Домен", "Domain", language)}</th>
              <th className="px-3 py-2 font-medium">{tr("Статус", "Status", language)}</th>
              <th className="px-3 py-2 font-medium">{tr("Срок", "Expires", language)}</th>
              <th className="px-3 py-2 font-medium">{tr("Provider", "Provider", language)}</th>
              <th className="px-3 py-2 font-medium">{tr("Действия", "Actions", language)}</th>
            </tr>
          </thead>
          <tbody>
            {certs.length === 0 ? (
              <tr>
                <td colSpan={5} className="px-3 py-4 text-slate-500">
                  {tr("Нет записей в metadata DB для этого project_id.", "No rows in metadata DB for this project_id.", language)}
                </td>
              </tr>
            ) : (
              certs.map((c) => {
                const st = statusLabel(c.status, language);
                return (
                  <tr key={c.primary_domain + c.cert_name} className="border-b border-white/5 hover:bg-white/[0.03]">
                    <td className="px-3 py-2 font-mono text-amber-200/90">{c.primary_domain}</td>
                    <td className={`px-3 py-2 ${st.className}`}>
                      <span className="inline-flex items-center gap-1">
                        <st.Icon className="h-3.5 w-3.5" />
                        {st.text}
                      </span>
                    </td>
                    <td className="px-3 py-2 text-slate-300">{formatExpiry(c.expiry_unix_ms, language)}</td>
                    <td className="px-3 py-2 text-slate-400">Certbot</td>
                    <td className="px-3 py-2">
                      <div className="flex flex-wrap gap-1">
                        <button
                          type="button"
                          className="rounded-lg border border-white/10 bg-white/5 px-2 py-1 text-[11px] text-slate-200 hover:bg-white/10"
                          onClick={() => setDetail(c)}
                        >
                          {tr("Подробно", "Details", language)}
                        </button>
                        <button
                          type="button"
                          className="rounded-lg border border-white/10 bg-white/5 px-2 py-1 text-[11px] text-slate-200 hover:bg-white/10"
                          onClick={() => setUpdateTarget(c)}
                        >
                          Renew
                        </button>
                      </div>
                    </td>
                  </tr>
                );
              })
            )}
          </tbody>
        </table>
      </div>

      {logLines?.length ? (
        <CopyablePre
          value={logLines.join("\n")}
          placeholder="—"
          className="rounded-xl border border-white/10 bg-black/40 p-3 text-xs text-slate-200"
          maxHeightClass="max-h-40"
        />
      ) : null}

      <div className="rounded-xl border border-white/10 bg-black/25 p-3">
        <p className="text-sm font-semibold text-slate-100">
          {tr("Планировщик renew (host deploy env)", "Renew scheduler (host deploy env)", language)}
        </p>
        <p className="mt-1 text-xs text-slate-500">
          {tr(
            "Ключи SSL_CHECK_INTERVAL (сек) и SSL_ENABLE_SCHEDULER (0/1) — те же, что в pirate-deploy на сервере.",
            "Keys SSL_CHECK_INTERVAL (sec) and SSL_ENABLE_SCHEDULER (0/1) — same as pirate-deploy on the host.",
            language,
          )}
        </p>
        <div className="mt-3 grid gap-2 sm:grid-cols-2">
          <label className="block text-xs text-slate-500">
            SSL_CHECK_INTERVAL
            <input
              value={schedInterval}
              onChange={(e) => setSchedInterval(e.target.value.replace(/[^\d]/g, ""))}
              className="mt-1 w-full rounded-lg border border-white/10 bg-black/30 px-2 py-1.5 font-mono text-slate-100"
            />
          </label>
          <label className="block text-xs text-slate-500">
            SSL_ENABLE_SCHEDULER
            <select
              value={schedEnable}
              onChange={(e) => setSchedEnable(e.target.value)}
              className="mt-1 w-full rounded-lg border border-white/10 bg-black/30 px-2 py-1.5 text-slate-100"
            >
              <option value="0">0 — off</option>
              <option value="1">1 — on</option>
            </select>
          </label>
        </div>
        <div className="mt-2 flex flex-wrap gap-2">
          <button
            type="button"
            disabled={schedBusy}
            onClick={() => void loadSchedulerEnv()}
            className={`${btnBase} border border-white/15 bg-white/5 px-3 py-2 text-xs text-slate-200 hover:bg-white/10`}
          >
            {schedBusy ? <Loader2 className="h-3.5 w-3.5 animate-spin" /> : null}
            {tr("Перечитать env", "Reload env", language)}
          </button>
          <button
            type="button"
            disabled={schedBusy}
            onClick={() => void saveSchedulerEnv()}
            className={`${btnBase} bg-gradient-to-r from-red-800/90 to-red-950/90 px-3 py-2 text-xs text-white`}
          >
            {tr("Сохранить в host env", "Save to host env", language)}
          </button>
        </div>
      </div>

      <div className="rounded-xl border border-amber-900/30 bg-amber-950/20 p-3 text-xs text-amber-100/90">
        <p className="font-semibold text-amber-200">
          {tr("Cloudflare / DNS-01 (далее)", "Cloudflare / DNS-01 (later)", language)}
        </p>
        <p className="mt-1 text-amber-100/75">
          {tr(
            "Полноценная интеграция (токен, keyring, credentials для certbot на хосте) вынесена в отдельный этап. Сейчас используйте SSL_MODE=dns и переменные SSL_CERTBOT_* на сервере.",
            "Full integration (token, keyring, certbot credentials on the host) is a separate phase. Use SSL_MODE=dns and SSL_CERTBOT_* on the server for now.",
            language,
          )}
        </p>
      </div>

      {addOpen ? (
        <ModalDialog open zClassName="z-modalServerSettings" onClose={() => setAddOpen(false)} panelClassName="w-full max-w-lg">
          <div className="max-h-[85vh] overflow-y-auto rounded-2xl border border-white/10 bg-[#0a0908] p-4 shadow-2xl">
            <h3 className="text-sm font-semibold text-slate-100">
              {tr("Новый сертификат (certbot)", "New certificate (certbot)", language)}
            </h3>
            <p className="mt-1 text-xs text-slate-500">
              {tr("Домены через пробел, запятую или с новой строки.", "Domains: space, comma, or line breaks.", language)}
            </p>
            <textarea
              value={addDomains}
              onChange={(e) => setAddDomains(e.target.value)}
              rows={3}
              className="mt-2 w-full rounded-lg border border-white/10 bg-black/35 px-2 py-1.5 font-mono text-sm text-slate-100"
              placeholder="example.com www.example.com"
            />
            <label className="mt-2 block text-xs text-slate-500">
              Mode
              <select
                value={addMode}
                onChange={(e) => setAddMode(Number(e.target.value))}
                className="mt-1 w-full rounded-lg border border-white/10 bg-black/30 px-2 py-1.5 text-slate-100"
              >
                {SSL_MODE_OPTIONS.map((o) => (
                  <option key={o.v} value={o.v}>
                    {language === "ru" ? o.ru : o.en}
                  </option>
                ))}
              </select>
            </label>
            <label className="mt-2 block text-xs text-slate-500">
              {tr("Webroot (для webroot)", "Webroot (for webroot mode)", language)}
              <input
                value={addWebroot}
                onChange={(e) => setAddWebroot(e.target.value)}
                className="mt-1 w-full rounded-lg border border-white/10 bg-black/30 px-2 py-1.5 font-mono text-sm text-slate-100"
              />
            </label>
            <div className="mt-2 flex flex-wrap gap-3 text-xs text-slate-400">
              <label className="flex items-center gap-2">
                <input type="checkbox" checked={addDry} onChange={(e) => setAddDry(e.target.checked)} className="rounded" />
                dry-run
              </label>
              <label className="flex items-center gap-2">
                <input type="checkbox" checked={addStaging} onChange={(e) => setAddStaging(e.target.checked)} className="rounded" />
                Let’s Encrypt staging
              </label>
            </div>
            <div className="mt-3 flex justify-end gap-2">
              <button
                type="button"
                onClick={() => setAddOpen(false)}
                className={`${btnBase} border border-white/15 bg-white/5 px-3 py-2 text-xs`}
              >
                {tr("Отмена", "Cancel", language)}
              </button>
              <button
                type="button"
                disabled={busy}
                onClick={() => void submitAdd()}
                className={`${btnBase} bg-gradient-to-r from-red-700 to-red-900 px-3 py-2 text-xs text-white`}
              >
                {tr("Выпустить", "Run", language)}
              </button>
            </div>
          </div>
        </ModalDialog>
      ) : null}

      {detail ? (
        <ModalDialog open zClassName="z-modalServerSettings" onClose={() => setDetail(null)} panelClassName="w-full max-w-lg">
          <div className="max-h-[85vh] overflow-y-auto rounded-2xl border border-white/10 bg-[#0a0908] p-4">
            <h3 className="text-sm font-semibold text-slate-100">
              {tr("Сертификат", "Certificate", language)}: {detail.primary_domain}
            </h3>
            <ul className="mt-2 space-y-1 text-xs text-slate-300">
              <li>
                {tr("Санчаты", "SANs", language)}: {detail.domains.join(", ") || "—"}
              </li>
              <li>live_path: {detail.live_path || "—"}</li>
              <li>cert_name: {detail.cert_name || "—"}</li>
              <li className="text-rose-200/90">
                last_error: {detail.last_error || "—"}
              </li>
            </ul>
            <button
              type="button"
              className={`${btnBase} mt-3 border border-white/15 bg-white/5 px-3 py-1.5 text-xs`}
              onClick={() => setDetail(null)}
            >
              {tr("Закрыть", "Close", language)}
            </button>
          </div>
        </ModalDialog>
      ) : null}

      {updateTarget ? (
        <ModalDialog
          open
          zClassName="z-modalServerSettings"
          onClose={() => setUpdateTarget(null)}
          panelClassName="w-full max-w-md"
        >
          <div className="rounded-2xl border border-white/10 bg-[#0a0908] p-4">
            <h3 className="text-sm font-semibold text-slate-100">SslUpdate — {updateTarget.primary_domain}</h3>
            <p className="mt-1 text-xs text-slate-500">exact_domain selector, dry_run optional.</p>
            <label className="mt-2 flex items-center gap-2 text-xs text-slate-400">
              <input type="checkbox" checked={updDry} onChange={(e) => setUpdDry(e.target.checked)} className="rounded" />
              dry_run
            </label>
            <div className="mt-3 flex justify-end gap-2">
              <button
                type="button"
                onClick={() => setUpdateTarget(null)}
                className={`${btnBase} border border-white/15 bg-white/5 px-3 py-1.5 text-xs`}
              >
                {tr("Отмена", "Cancel", language)}
              </button>
              <button
                type="button"
                disabled={busy}
                onClick={() => void submitUpdate()}
                className={`${btnBase} bg-gradient-to-r from-red-700 to-red-900 px-3 py-1.5 text-xs text-white`}
              >
                {tr("Запустить", "Run", language)}
              </button>
            </div>
          </div>
        </ModalDialog>
      ) : null}
    </div>
  );
}
