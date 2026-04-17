/**
 * Anti-DDoS host settings (GET/PUT /api/v1/antiddos) via control-api JWT.
 */
import { invoke } from "@tauri-apps/api/core";
import { Loader2, RefreshCw, Shield } from "lucide-react";
import React, { useCallback, useEffect, useState } from "react";
import { toast } from "sonner";
import { useI18n } from "./i18n";

const btnSm =
  "inline-flex items-center justify-center gap-1.5 rounded-lg px-2.5 py-1.5 text-xs font-semibold transition focus-visible:outline-none focus-visible:ring-2 focus-visible:ring-red-600/80 disabled:pointer-events-none disabled:opacity-50";

type AntiddosHostConfig = {
  schema_version: number;
  engine: string;
  enabled: boolean;
  aggressive: boolean;
  rate_limit_rps: number;
  burst: number;
  max_connections_per_ip: number;
  client_body_timeout_sec: number;
  keepalive_timeout_sec: number;
  send_timeout_sec: number;
  whitelist_cidrs: string[];
  fail2ban: { enabled: boolean; bantime_sec: number; findtime_sec: number; maxretry: number };
  firewall: { enabled: boolean; syn_tuning: boolean };
  lockdown_app_ports: { enabled: boolean; tcp_ports: number[] };
};

type AntiddosGetResponse = {
  config: AntiddosHostConfig;
  last_apply?: { ok: boolean; message: string; stderr?: string | null };
};

function defaultConfig(): AntiddosHostConfig {
  return {
    schema_version: 1,
    engine: "nginx_nft_fail2ban",
    enabled: false,
    aggressive: false,
    rate_limit_rps: 10,
    burst: 20,
    max_connections_per_ip: 30,
    client_body_timeout_sec: 12,
    keepalive_timeout_sec: 20,
    send_timeout_sec: 10,
    whitelist_cidrs: ["127.0.0.1/32", "::1/128"],
    fail2ban: { enabled: true, bantime_sec: 600, findtime_sec: 120, maxretry: 10 },
    firewall: { enabled: true, syn_tuning: true },
    lockdown_app_ports: { enabled: false, tcp_ports: [] },
  };
}

export function AntiDdosPanel({ sessionOk }: { sessionOk: boolean }) {
  const { language } = useI18n();
  /** Stable per language only — do not close over a new inline fn each render (would break useCallback/useEffect). */
  const tr = (ru: string, en: string) => (language === "ru" ? ru : en);
  const [cfg, setCfg] = useState<AntiddosHostConfig>(defaultConfig);
  const [whitelistText, setWhitelistText] = useState("127.0.0.1/32\n::1/128");
  const [lockdownPorts, setLockdownPorts] = useState("");
  const [loading, setLoading] = useState(false);
  const [busy, setBusy] = useState(false);
  const [lastApply, setLastApply] = useState<string | null>(null);
  const [stats, setStats] = useState<string | null>(null);

  const syncFromConfig = useCallback((c: AntiddosHostConfig) => {
    setCfg(c);
    setWhitelistText((c.whitelist_cidrs || []).join("\n"));
    setLockdownPorts((c.lockdown_app_ports?.tcp_ports || []).join(", "));
  }, []);

  const load = useCallback(async () => {
    setLoading(true);
    setLastApply(null);
    try {
      const j = await invoke<string>("control_api_antiddos_get_json");
      const parsed = JSON.parse(j) as AntiddosGetResponse;
      syncFromConfig(parsed.config);
      if (parsed.last_apply) {
        setLastApply(JSON.stringify(parsed.last_apply, null, 2));
      }
    } catch (e) {
      const msg = language === "ru" ? "Не удалось загрузить" : "Load failed";
      toast.error(msg, { description: String(e) });
    } finally {
      setLoading(false);
    }
  }, [syncFromConfig, language]);

  useEffect(() => {
    if (sessionOk) void load();
  }, [sessionOk, load]);

  const save = async () => {
    const wl = whitelistText
      .split("\n")
      .map((s) => s.trim())
      .filter(Boolean);
    const ports = lockdownPorts
      .split(/[,\s]+/)
      .map((s) => parseInt(s.trim(), 10))
      .filter((n) => !Number.isNaN(n) && n > 0 && n <= 65535);
    const next: AntiddosHostConfig = {
      ...cfg,
      whitelist_cidrs: wl,
      lockdown_app_ports: { ...cfg.lockdown_app_ports, tcp_ports: ports },
    };
    setBusy(true);
    try {
      const body = JSON.stringify(next);
      const r = await invoke<string>("control_api_antiddos_put_json", { content: body });
      setLastApply(r);
      toast.success(tr("Применено", "Applied"));
      await load();
    } catch (e) {
      toast.error(tr("Ошибка", "Error"), { description: String(e) });
    } finally {
      setBusy(false);
    }
  };

  const runEnable = async () => {
    setBusy(true);
    try {
      const r = await invoke<string>("control_api_antiddos_enable");
      setLastApply(r);
      toast.success(tr("Включено", "Enabled"));
      await load();
    } catch (e) {
      toast.error(tr("Ошибка", "Error"), { description: String(e) });
    } finally {
      setBusy(false);
    }
  };

  const runDisable = async () => {
    setBusy(true);
    try {
      const r = await invoke<string>("control_api_antiddos_disable");
      setLastApply(r);
      toast.success(tr("Выключено", "Disabled"));
      await load();
    } catch (e) {
      toast.error(tr("Ошибка", "Error"), { description: String(e) });
    } finally {
      setBusy(false);
    }
  };

  const runStats = async () => {
    setBusy(true);
    try {
      const s = await invoke<string>("control_api_antiddos_stats_json");
      setStats(s);
    } catch (e) {
      setStats(String(e));
    } finally {
      setBusy(false);
    }
  };

  if (!sessionOk) {
    return (
      <p className="text-sm text-slate-500">
        {tr("Сначала войдите на вкладке «Подключение».", "Sign in on the Connection tab first.")}
      </p>
    );
  }

  return (
    <div className="space-y-4">
      <div className="flex flex-wrap items-center gap-2">
        <Shield className="h-4 w-4 text-amber-400/90" aria-hidden />
        <span className="text-sm font-medium text-slate-200">
          {tr("Anti-DDoS (nginx + nft + fail2ban)", "Anti-DDoS (nginx + nft + fail2ban)")}
        </span>
        <button
          type="button"
          disabled={loading}
          onClick={() => void load()}
          className={`${btnSm} border border-white/10 bg-white/5 text-slate-200 hover:bg-white/10`}
        >
          {loading ? <Loader2 className="h-3.5 w-3.5 animate-spin" /> : <RefreshCw className="h-3.5 w-3.5" />}
          {tr("Обновить", "Refresh")}
        </button>
        <button
          type="button"
          disabled={busy}
          onClick={() => void runStats()}
          className={`${btnSm} border border-white/10 bg-white/5 text-slate-200 hover:bg-white/10`}
        >
          {tr("Статистика", "Stats")}
        </button>
      </div>
      <p className="text-xs text-slate-500">
        {tr(
          "Лимиты L7 в nginx (limit_req / limit_conn), nft только для 80/443, fail2ban по логу. SSH не трогаем.",
          "L7 limits in nginx (limit_req / limit_conn), nft for 80/443 only, fail2ban on the limit log. SSH is untouched.",
        )}
      </p>

      <label className="flex cursor-pointer items-center gap-2 text-sm text-slate-300">
        <input
          type="checkbox"
          checked={cfg.enabled}
          onChange={(e) => setCfg({ ...cfg, enabled: e.target.checked })}
          className="rounded border-white/20 bg-black/40"
        />
        {tr("Включить защиту на хосте", "Enable protection on host")}
      </label>
      <label className="flex cursor-pointer items-center gap-2 text-sm text-slate-300">
        <input
          type="checkbox"
          checked={cfg.aggressive}
          onChange={(e) => setCfg({ ...cfg, aggressive: e.target.checked })}
          className="rounded border-white/20 bg-black/40"
        />
        {tr("Агрессивный режим (ужесточить лимиты)", "Aggressive mode (tighter limits)")}
      </label>

      <div className="grid grid-cols-2 gap-3 sm:grid-cols-3">
        {(
          [
            ["rate_limit_rps", tr("Запросов/с (r/s)", "Requests/s"), cfg.rate_limit_rps],
            ["burst", "burst", cfg.burst],
            ["max_connections_per_ip", tr("conn/IP", "conn/IP"), cfg.max_connections_per_ip],
          ] as const
        ).map(([k, label, val]) => (
          <label key={k} className="text-xs text-slate-400">
            {label}
            <input
              type="number"
              className="mt-1 w-full rounded-lg border border-white/10 bg-black/35 px-2 py-1.5 text-slate-100"
              value={val}
              onChange={(e) => {
                const v = parseFloat(e.target.value);
                if (k === "rate_limit_rps") setCfg({ ...cfg, rate_limit_rps: v });
                else if (k === "burst") setCfg({ ...cfg, burst: Math.floor(v) });
                else setCfg({ ...cfg, max_connections_per_ip: Math.floor(v) });
              }}
            />
          </label>
        ))}
      </div>

      <div>
        <p className="mb-1 text-xs font-medium text-slate-400">{tr("Whitelist CIDR", "Whitelist CIDRs")}</p>
        <textarea
          value={whitelistText}
          onChange={(e) => setWhitelistText(e.target.value)}
          rows={4}
          className="w-full rounded-xl border border-white/10 bg-black/35 px-3 py-2 font-mono text-xs text-slate-100"
          spellCheck={false}
        />
      </div>

      <div className="grid grid-cols-2 gap-3 border-t border-white/10 pt-3">
        <label className="flex cursor-pointer items-center gap-2 text-xs text-slate-400">
          <input
            type="checkbox"
            checked={cfg.fail2ban.enabled}
            onChange={(e) =>
              setCfg({ ...cfg, fail2ban: { ...cfg.fail2ban, enabled: e.target.checked } })
            }
          />
          fail2ban
        </label>
        <label className="flex cursor-pointer items-center gap-2 text-xs text-slate-400">
          <input
            type="checkbox"
            checked={cfg.firewall.enabled}
            onChange={(e) =>
              setCfg({ ...cfg, firewall: { ...cfg.firewall, enabled: e.target.checked } })
            }
          />
          nft firewall
        </label>
        <label className="flex cursor-pointer items-center gap-2 text-xs text-slate-400">
          <input
            type="checkbox"
            checked={cfg.lockdown_app_ports.enabled}
            onChange={(e) =>
              setCfg({
                ...cfg,
                lockdown_app_ports: { ...cfg.lockdown_app_ports, enabled: e.target.checked },
              })
            }
          />
          {tr("Закрыть backend-порты с WAN", "Lock backend ports from WAN")}
        </label>
      </div>
      <div>
        <p className="mb-1 text-xs text-slate-500">
          {tr("Порты lockdown (через запятую)", "Lockdown TCP ports (comma-separated)")}
        </p>
        <input
          value={lockdownPorts}
          onChange={(e) => setLockdownPorts(e.target.value)}
          className="w-full rounded-lg border border-white/10 bg-black/35 px-2 py-1.5 font-mono text-xs text-slate-100"
          placeholder="3000, 8080"
        />
      </div>

      <div className="flex flex-wrap gap-2">
        <button
          type="button"
          disabled={busy}
          onClick={() => void save()}
          className={`${btnSm} bg-gradient-to-r from-red-700 to-red-900 text-white shadow-lg shadow-red-950/40 hover:brightness-110 disabled:opacity-40`}
        >
          {busy ? <Loader2 className="h-3.5 w-3.5 animate-spin" /> : null}
          {tr("Сохранить и применить", "Save & apply")}
        </button>
        <button
          type="button"
          disabled={busy}
          onClick={() => void runEnable()}
          className={`${btnSm} border border-emerald-500/40 bg-emerald-950/40 text-emerald-300 hover:bg-emerald-900/40`}
        >
          {tr("Только включить", "Enable only")}
        </button>
        <button
          type="button"
          disabled={busy}
          onClick={() => void runDisable()}
          className={`${btnSm} border border-white/10 bg-white/5 text-slate-200 hover:bg-white/10`}
        >
          {tr("Только выключить", "Disable only")}
        </button>
      </div>

      {lastApply ? (
        <pre className="max-h-32 overflow-auto rounded-lg border border-white/10 bg-black/40 p-2 text-[10px] text-slate-400">
          {lastApply}
        </pre>
      ) : null}
      {stats ? (
        <pre className="max-h-40 overflow-auto rounded-lg border border-white/10 bg-black/40 p-2 text-[10px] text-slate-400">
          {stats}
        </pre>
      ) : null}
    </div>
  );
}
