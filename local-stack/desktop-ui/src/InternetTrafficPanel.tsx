/**
 * Локальный HTTP CONNECT → gRPC ProxyTunnel (тот же движок, что `pirate board`).
 * Для HTTPS через CONNECT решение принимается по имени хоста (не по URL path внутри TLS).
 */
import { invoke } from "@tauri-apps/api/core";
import { Globe, Loader2, Power, ScrollText, X } from "lucide-react";
import React, { useCallback, useEffect, useState } from "react";

type InternetProxyStatus = {
  running: boolean;
  listen: string;
  lastError: string | null;
};

type RuleBundleEdit = {
  domains: string[];
  domainPatterns: string[];
  ips: string[];
  categoriesOur?: unknown;
};

type DefaultRulesBundlesForm = {
  block: RuleBundleEdit;
  pass: RuleBundleEdit;
  our: RuleBundleEdit;
};

type TrafficRuleSourceUi = "merged" | "bundles" | "board";

type ProxyTraceEntry = {
  timestampMs: number;
  clientAddr: string;
  target: string;
  decision: string;
  route: string;
  result: string;
  detail?: string | null;
};

type BoardRulesForm = {
  trafficRuleSource: string;
  defaultBoard: string;
  boardId: string;
  globalBypass: string[];
  bypass: string[];
};

const btnBase =
  "inline-flex items-center justify-center gap-2 rounded-xl px-4 py-2.5 text-sm font-semibold transition-all duration-200 focus-visible:outline-none focus-visible:ring-2 focus-visible:ring-red-600/80 focus-visible:ring-offset-2 focus-visible:ring-offset-[#050204] active:scale-[0.98] disabled:pointer-events-none disabled:opacity-50";

const PRESETS: { id: string; label: string }[] = [
  { id: "combined", label: "Все три: anti-adw + ru-full + ru-block-domain" },
  { id: "anti-adw-only", label: "Блокировка рекламы (anti-adw)" },
  { id: "ru-full-only", label: "Все домены .ru пропускаются (ru-full)" },
  { id: "ru-block-domain-only", label: "Доступ к заблокированным доменам (ru-block-domain)" },
];

/** Matches `default_listen_addr()` in desktop-client `internet_proxy`. */
const DEFAULT_PROXY_LISTEN = "127.0.0.1:3128";

type RulesTab = "block" | "pass" | "our";

function linesToArr(s: string): string[] {
  return s
    .split("\n")
    .map((l) => l.trim())
    .filter(Boolean);
}

export function InternetTrafficPanel() {
  const [status, setStatus] = useState<InternetProxyStatus | null>(null);
  const [busy, setBusy] = useState(false);
  const [settingsText, setSettingsText] = useState("");
  const [settingsMsg, setSettingsMsg] = useState<string | null>(null);
  const [preset, setPreset] = useState("combined");

  const [bundlesForm, setBundlesForm] = useState<DefaultRulesBundlesForm | null>(null);
  const [rulesTab, setRulesTab] = useState<RulesTab>("block");
  const [categoriesOurText, setCategoriesOurText] = useState("");
  const [rulesMsg, setRulesMsg] = useState<string | null>(null);

  const [boardForm, setBoardForm] = useState<BoardRulesForm | null>(null);
  const [boardMsg, setBoardMsg] = useState<string | null>(null);

  const [logsOpen, setLogsOpen] = useState(false);
  const [proxyLogs, setProxyLogs] = useState<ProxyTraceEntry[]>([]);

  const trafficSourceNormalized = (raw: string | undefined): TrafficRuleSourceUi => {
    const t = (raw ?? "").trim().toLowerCase();
    if (t === "bundles" || t === "board") return t;
    return "merged";
  };

  const refreshStatus = useCallback(async () => {
    try {
      const s = await invoke<InternetProxyStatus>("internet_proxy_status");
      setStatus(s);
    } catch {
      setStatus(null);
    }
  }, []);

  const refreshProxyLogs = useCallback(async () => {
    try {
      const rows = await invoke<ProxyTraceEntry[]>("internet_proxy_logs");
      setProxyLogs(rows);
    } catch {
      setProxyLogs([]);
    }
  }, []);

  const loadSettings = useCallback(async () => {
    setSettingsMsg(null);
    try {
      const t = await invoke<string>("load_client_settings_json");
      setSettingsText(t);
    } catch (e) {
      setSettingsMsg(String(e));
    }
  }, []);

  const loadRulesForm = useCallback(async () => {
    setRulesMsg(null);
    try {
      const f = await invoke<DefaultRulesBundlesForm>("load_default_rules_bundles_form");
      setBundlesForm(f);
      setCategoriesOurText(
        f.our.categoriesOur != null ? JSON.stringify(f.our.categoriesOur, null, 2) : "",
      );
    } catch (e) {
      setRulesMsg(String(e));
    }
  }, []);

  const loadBoardForm = useCallback(async () => {
    setBoardMsg(null);
    try {
      const f = await invoke<BoardRulesForm>("load_board_rules_form");
      setBoardForm(f);
    } catch (e) {
      setBoardMsg(String(e));
    }
  }, []);

  useEffect(() => {
    void refreshStatus();
    void loadSettings();
    void loadRulesForm();
    void loadBoardForm();
    const id = window.setInterval(() => void refreshStatus(), 4000);
    return () => window.clearInterval(id);
  }, [refreshStatus, loadSettings, loadRulesForm, loadBoardForm]);

  useEffect(() => {
    if (!logsOpen) return;
    void refreshProxyLogs();
    const id = window.setInterval(() => void refreshProxyLogs(), 1000);
    return () => window.clearInterval(id);
  }, [logsOpen, refreshProxyLogs]);

  const patchBundle = useCallback(
    (which: RulesTab, patch: Partial<Pick<RuleBundleEdit, "domains" | "domainPatterns" | "ips">>) => {
      setBundlesForm((prev) => {
        if (!prev) return prev;
        return {
          ...prev,
          [which]: { ...prev[which], ...patch },
        };
      });
    },
    [],
  );

  const onToggleProxy = async () => {
    if (status?.running) {
      setBusy(true);
      try {
        await invoke("internet_proxy_stop");
        await refreshStatus();
      } finally {
        setBusy(false);
      }
      return;
    }
    setBusy(true);
    try {
      await invoke("internet_proxy_start", { listen: null });
      await refreshStatus();
    } catch (e) {
      setStatus((prev) =>
        prev
          ? { ...prev, lastError: String(e) }
          : {
              running: false,
              listen: DEFAULT_PROXY_LISTEN,
              lastError: String(e),
            },
      );
    } finally {
      setBusy(false);
    }
  };

  const onClearProxyLogs = async () => {
    try {
      await invoke("internet_proxy_logs_clear");
      await refreshProxyLogs();
    } catch {
      /* ignore */
    }
  };

  const formatProxyLogLine = (e: ProxyTraceEntry): string => {
    const t = new Date(e.timestampMs).toLocaleTimeString(undefined, {
      hour12: false,
      fractionalSecondDigits: 3,
    });
    const res = e.result.toUpperCase();
    const det = e.detail?.trim() ? ` | ${e.detail}` : "";
    return `[${t}] ${e.clientAddr} | ${e.target} | ${e.decision} | ${e.route} | ${res}${det}`;
  };

  const onSaveSettings = async () => {
    setSettingsMsg(null);
    setBusy(true);
    try {
      await invoke("save_client_settings_json", { text: settingsText });
      setSettingsMsg("Сохранено в settings.json (как у CLI pirate).");
    } catch (e) {
      setSettingsMsg(String(e));
    } finally {
      setBusy(false);
    }
  };

  const onApplyPreset = async () => {
    setSettingsMsg(null);
    setRulesMsg(null);
    setBusy(true);
    try {
      await invoke("apply_default_rules_preset_cmd", { preset });
      await loadSettings();
      await loadRulesForm();
      setSettingsMsg("Пресет применён (файлы default-rules перезаписаны).");
    } catch (e) {
      setSettingsMsg(String(e));
    } finally {
      setBusy(false);
    }
  };

  const onSaveRulesBundles = async () => {
    if (!bundlesForm) return;
    setRulesMsg(null);
    setBusy(true);
    try {
      const t = categoriesOurText.trim();
      let categoriesOur: unknown = undefined;
      if (t) {
        try {
          categoriesOur = JSON.parse(t) as unknown;
        } catch {
          setRulesMsg("categories_our: невалидный JSON");
          setBusy(false);
          return;
        }
      }
      const form: DefaultRulesBundlesForm = {
        ...bundlesForm,
        our: { ...bundlesForm.our, categoriesOur },
      };
      await invoke("save_default_rules_bundles_form", { form });
      setRulesMsg("Правила сохранены (user-*.json + пути в settings).");
      await loadSettings();
      await loadRulesForm();
    } catch (e) {
      setRulesMsg(String(e));
    } finally {
      setBusy(false);
    }
  };

  const onSaveBoard = async () => {
    if (!boardForm) return;
    setBoardMsg(null);
    setBusy(true);
    try {
      await invoke("save_board_rules_form", { form: boardForm });
      setBoardMsg("Настройки доски сохранены.");
      await loadSettings();
    } catch (e) {
      setBoardMsg(String(e));
    } finally {
      setBusy(false);
    }
  };

  const proxyUrl = `http://${(status?.listen ?? DEFAULT_PROXY_LISTEN).replace(/^https?:\/\//, "")}`;

  const curBundle = bundlesForm
    ? bundlesForm[rulesTab]
    : null;

  const ruleSource = trafficSourceNormalized(boardForm?.trafficRuleSource);

  return (
    <section
      className="rounded-2xl border border-white/10 bg-surface/90 p-5 shadow-card backdrop-blur"
      aria-labelledby="internet-traffic-heading"
    >
      <div className="flex items-start gap-3">
        <div className="flex h-10 w-10 shrink-0 items-center justify-center rounded-xl bg-red-950/50 text-red-200">
          <Globe className="h-5 w-5" aria-hidden />
        </div>
        <div className="min-w-0 flex-1">
          <h2
            id="internet-traffic-heading"
            className="text-sm font-semibold tracking-tight text-slate-100"
          >
            Интернет-трафик через клиент
          </h2>
          <p className="mt-1 text-xs leading-relaxed text-slate-500">
            Локальный прокси HTTP CONNECT на этом Mac; трафик, отправляемый на сервер, идёт по
            подписанному gRPC <code className="text-slate-400">ProxyTunnel</code>. Для HTTPS
            правила смотрят только <strong className="text-slate-400">имя хоста</strong> из CONNECT,
            не путь и не тело запроса.
          </p>
        </div>
      </div>

      <div className="mt-4 flex flex-wrap items-center gap-3">
        <span className="text-xs uppercase tracking-wide text-slate-500">Статус</span>
        <span
          className={`rounded-lg px-2 py-1 text-xs font-medium ${
            status?.running ? "bg-emerald-950/60 text-emerald-300" : "bg-black/30 text-slate-400"
          }`}
        >
          {status?.running ? "Запущен" : "Остановлен"}
        </span>
        {status?.lastError ? (
          <span className="max-w-full truncate text-xs text-rose-300" title={status.lastError}>
            {status.lastError}
          </span>
        ) : null}
      </div>

      <div className="mt-4 flex flex-wrap items-center gap-2">
        <button
          type="button"
          onClick={() => void onToggleProxy()}
          disabled={busy}
          className={
            status?.running
              ? `${btnBase} flex-1 min-w-[10rem] border border-white/15 bg-black/40 text-slate-200 hover:bg-black/55`
              : `${btnBase} flex-1 min-w-[10rem] border border-emerald-700/50 bg-emerald-950/40 text-emerald-100 hover:bg-emerald-950/60`
          }
        >
          {busy ? <Loader2 className="h-4 w-4 animate-spin" /> : <Power className="h-4 w-4" />}
          {status?.running ? "Остановить" : "Запустить"}
        </button>
        <button
          type="button"
          onClick={() => {
            setLogsOpen(true);
            void refreshProxyLogs();
          }}
          className={`${btnBase} shrink-0 border border-slate-600/50 bg-slate-900/50 text-slate-200 hover:bg-slate-900/70`}
          title="Журнал CONNECT: откуда, правило, маршрут, OK/FAIL"
        >
          <ScrollText className="h-4 w-4" />
          Логи прокси
        </button>
      </div>

      <div className="mt-5 rounded-xl border border-white/10 bg-black/25 p-3 text-xs text-slate-400">
        <p className="font-medium text-slate-300">Переменные окружения (терминал / приложения)</p>
        <pre className="mt-2 overflow-x-auto whitespace-pre-wrap break-all font-mono text-[11px] text-amber-100/80">
          {`export HTTPS_PROXY=${proxyUrl}
export HTTP_PROXY=${proxyUrl}`}
        </pre>
        <p className="mt-2 text-slate-500">
          В System Settings → Network → Details → Proxies можно задать веб-прокси на этот адрес.
        </p>
      </div>

      <div className="mt-6 border-t border-white/10 pt-5">
        <h3 className="text-xs font-medium uppercase tracking-wide text-slate-500">
          Пресеты правил (как server-stack/default-rules)
        </h3>
        <div className="mt-2 flex flex-col gap-2 sm:flex-row sm:items-center">
          <select
            className="w-full rounded-xl border border-white/10 bg-black/30 px-3 py-2 text-sm text-slate-100 focus:border-red-600 focus:outline-none sm:max-w-md"
            value={preset}
            onChange={(e) => setPreset(e.target.value)}
            disabled={busy}
          >
            {PRESETS.map((p) => (
              <option key={p.id} value={p.id}>
                {p.label}
              </option>
            ))}
          </select>
          <button
            type="button"
            onClick={() => void onApplyPreset()}
            disabled={busy}
            className={`${btnBase} shrink-0 border border-red-600/50 bg-red-950/40 text-red-100 hover:bg-red-950/60`}
          >
            Применить пресет
          </button>
        </div>
        <p className="mt-2 text-xs text-slate-500">
          Перезаписывает файлы в default-rules. Свои правила ниже сохраняются в user-*.json — после
          пресета снова нажмите «Сохранить набор правил», если нужны свои файлы.
        </p>
      </div>

      <div className="mt-6 border-t border-white/10 pt-5">
        <div className="flex flex-wrap items-center justify-between gap-2">
          <h3 className="text-xs font-medium uppercase tracking-wide text-slate-500">
            Редактор наборов правил (block / pass / our)
          </h3>
          <button
            type="button"
            onClick={() => void loadRulesForm()}
            disabled={busy}
            className={`${btnBase} border border-white/10 bg-black/30 px-3 py-1.5 text-xs text-slate-300`}
          >
            Перечитать
          </button>
        </div>
        <p className="mt-1 text-xs text-slate-500">
          Одна строка — один домен или паттерн. Сохранение записывает user-block.json / user-pass.json
          / user-our.json и обновляет пути в settings.
        </p>
        {ruleSource === "board" ? (
          <p className="mt-2 rounded-lg border border-amber-800/50 bg-amber-950/30 px-3 py-2 text-xs text-amber-100/90">
            Режим <code className="text-amber-200/90">board</code>: JSON-наборы ниже не участвуют в
            маршрутизации — используются только списки на доске и глобальный bypass (см. раздел ниже).
          </p>
        ) : null}
        <div className="mt-3 inline-flex rounded-xl border border-white/10 p-0.5">
          {(["block", "pass", "our"] as const).map((tab) => (
            <button
              key={tab}
              type="button"
              onClick={() => setRulesTab(tab)}
              className={`rounded-lg px-3 py-1.5 text-xs font-medium ${
                rulesTab === tab
                  ? "bg-red-900/60 text-white"
                  : "text-slate-400 hover:text-slate-200"
              }`}
            >
              {tab === "block" ? "Блок" : tab === "pass" ? "Пропуск" : "Our / split"}
            </button>
          ))}
        </div>
        {curBundle ? (
          <div className="mt-3 grid gap-3 md:grid-cols-3">
            <label className="block text-xs text-slate-500">
              Домены
              <textarea
                className="mt-1 min-h-[120px] w-full rounded-xl border border-white/10 bg-black/40 p-2 font-mono text-[11px] text-slate-200"
                value={curBundle.domains.join("\n")}
                onChange={(e) =>
                  patchBundle(rulesTab, { domains: linesToArr(e.target.value) })
                }
                spellCheck={false}
              />
            </label>
            <label className="block text-xs text-slate-500">
              Паттерны доменов
              <textarea
                className="mt-1 min-h-[120px] w-full rounded-xl border border-white/10 bg-black/40 p-2 font-mono text-[11px] text-slate-200"
                value={curBundle.domainPatterns.join("\n")}
                onChange={(e) =>
                  patchBundle(rulesTab, { domainPatterns: linesToArr(e.target.value) })
                }
                spellCheck={false}
              />
            </label>
            <label className="block text-xs text-slate-500">
              IP / CIDR
              <textarea
                className="mt-1 min-h-[120px] w-full rounded-xl border border-white/10 bg-black/40 p-2 font-mono text-[11px] text-slate-200"
                value={curBundle.ips.join("\n")}
                onChange={(e) => patchBundle(rulesTab, { ips: linesToArr(e.target.value) })}
                spellCheck={false}
              />
            </label>
          </div>
        ) : (
          <p className="mt-2 text-xs text-slate-500">Загрузка…</p>
        )}
        {rulesTab === "our" ? (
          <label className="mt-3 block text-xs text-slate-500">
            categories_our (JSON, опционально)
            <textarea
              className="mt-1 min-h-[80px] w-full rounded-xl border border-white/10 bg-black/40 p-2 font-mono text-[11px] text-slate-200"
              value={categoriesOurText}
              onChange={(e) => setCategoriesOurText(e.target.value)}
              spellCheck={false}
            />
          </label>
        ) : null}
        <div className="mt-3 flex flex-wrap items-center gap-2">
          <button
            type="button"
            onClick={() => void onSaveRulesBundles()}
            disabled={busy || !bundlesForm}
            className={`${btnBase} border border-amber-700/50 bg-amber-950/40 text-amber-100 hover:bg-amber-950/60`}
          >
            Сохранить набор правил
          </button>
          {rulesMsg ? <span className="text-xs text-slate-400">{rulesMsg}</span> : null}
        </div>
      </div>

      <div className="mt-6 border-t border-white/10 pt-5">
        <div className="flex flex-wrap items-center justify-between gap-2">
          <h3 className="text-xs font-medium uppercase tracking-wide text-slate-500">
            Доска и bypass (settings.json)
          </h3>
          <button
            type="button"
            onClick={() => void loadBoardForm()}
            disabled={busy}
            className={`${btnBase} border border-white/10 bg-black/30 px-3 py-1.5 text-xs text-slate-300`}
          >
            Перечитать
          </button>
        </div>
        <p className="mt-1 text-xs text-slate-500">
          Активная доска <code className="text-slate-400">boardId</code>, глобальный и локальный
          bypass, anti-adw, ru_block, not_ru_web — как в CLI.
        </p>
        {boardForm ? (
          <div className="mt-3 grid gap-3 sm:grid-cols-2">
            <label className="block text-xs text-slate-500 sm:col-span-2">
              Источник правил трафика{" "}
              <span className="font-normal text-slate-600">(global.traffic_rule_source)</span>
              <select
                className="mt-1 w-full rounded-xl border border-white/10 bg-black/30 px-3 py-2 text-sm text-slate-100"
                value={ruleSource}
                onChange={(e) =>
                  setBoardForm((b) =>
                    b
                      ? {
                          ...b,
                          trafficRuleSource: e.target.value,
                        }
                      : b,
                  )
                }
              >
                <option value="merged">merged — JSON и доска (как раньше)</option>
                <option value="bundles">bundles — только JSON default_rules + global.bypass</option>
                <option value="board">board — только списки доски + global.bypass</option>
              </select>
            </label>
            {ruleSource === "bundles" ? (
              <p className="text-xs text-slate-500 sm:col-span-2">
                В режиме bundles поля anti-adw, ru_block и bypass доски не влияют на решение CONNECT —
                их можно не заполнять.
              </p>
            ) : null}
            <label className="block text-xs text-slate-500">
              default_board
              <input
                className="mt-1 w-full rounded-xl border border-white/10 bg-black/30 px-3 py-2 text-sm text-slate-100"
                value={boardForm.defaultBoard}
                onChange={(e) =>
                  setBoardForm((b) => (b ? { ...b, defaultBoard: e.target.value } : b))
                }
              />
            </label>
            <label className="block text-xs text-slate-500">
              board_id (редактируемая доска)
              <input
                className="mt-1 w-full rounded-xl border border-white/10 bg-black/30 px-3 py-2 text-sm text-slate-100"
                value={boardForm.boardId}
                onChange={(e) =>
                  setBoardForm((b) => (b ? { ...b, boardId: e.target.value } : b))
                }
              />
            </label>
            <label className="block text-xs text-slate-500 sm:col-span-2">
              Глобальный bypass (строки)
              <textarea
                className="mt-1 min-h-[72px] w-full rounded-xl border border-white/10 bg-black/40 p-2 font-mono text-[11px] text-slate-200"
                value={boardForm.globalBypass.join("\n")}
                onChange={(e) =>
                  setBoardForm((b) =>
                    b ? { ...b, globalBypass: linesToArr(e.target.value) } : b,
                  )
                }
                spellCheck={false}
              />
            </label>
            {ruleSource !== "bundles" ? (
              <label className="block text-xs text-slate-500 sm:col-span-2">
                Bypass доски
                <textarea
                  className="mt-1 min-h-[72px] w-full rounded-xl border border-white/10 bg-black/40 p-2 font-mono text-[11px] text-slate-200"
                  value={boardForm.bypass.join("\n")}
                  onChange={(e) =>
                    setBoardForm((b) => (b ? { ...b, bypass: linesToArr(e.target.value) } : b))
                  }
                  spellCheck={false}
                />
              </label>
            ) : null}
            {/* {ruleSource !== "bundles" ? (
              <>
                <label className="flex items-center gap-2 text-xs text-slate-400">
                  <input
                    type="checkbox"
                    checked={boardForm.antiAdwEnabled}
                    onChange={(e) =>
                      setBoardForm((b) =>
                        b ? { ...b, antiAdwEnabled: e.target.checked } : b,
                      )
                    }
                  />
                  anti_adw_enabled
                </label>
                <label className="flex items-center gap-2 text-xs text-slate-400">
                  <input
                    type="checkbox"
                    checked={boardForm.ruBlockEnabled}
                    onChange={(e) =>
                      setBoardForm((b) =>
                        b ? { ...b, ruBlockEnabled: e.target.checked } : b,
                      )
                    }
                  />
                  ru_block_enabled
                </label>
              </>
            ) : null} */}
            {/* <label className="flex items-center gap-2 text-xs text-slate-400 sm:col-span-2">
              <input
                type="checkbox"
                checked={boardForm.notRuWebEnabled}
                onChange={(e) =>
                  setBoardForm((b) =>
                    b ? { ...b, notRuWebEnabled: e.target.checked } : b,
                  )
                }
              />
              not_ru_web_enabled
            </label> */}
            {/* {ruleSource !== "bundles" ? (
              <label className="block text-xs text-slate-500 sm:col-span-2">
                anti_adw
                <textarea
                  className="mt-1 min-h-[64px] w-full rounded-xl border border-white/10 bg-black/40 p-2 font-mono text-[11px] text-slate-200"
                  value={boardForm.antiAdw.join("\n")}
                  onChange={(e) =>
                    setBoardForm((b) => (b ? { ...b, antiAdw: linesToArr(e.target.value) } : b))
                  }
                  spellCheck={false}
                />
              </label>
            ) : null}
            {ruleSource !== "bundles" ? (
              <label className="block text-xs text-slate-500 sm:col-span-2">
                ru_block
                <textarea
                  className="mt-1 min-h-[64px] w-full rounded-xl border border-white/10 bg-black/40 p-2 font-mono text-[11px] text-slate-200"
                  value={boardForm.ruBlock.join("\n")}
                  onChange={(e) =>
                    setBoardForm((b) => (b ? { ...b, ruBlock: linesToArr(e.target.value) } : b))
                  }
                  spellCheck={false}
                />
              </label>
            ) : null} */}
            <div className="sm:col-span-2">
              <button
                type="button"
                onClick={() => void onSaveBoard()}
                disabled={busy}
                className={`${btnBase} border border-violet-700/50 bg-violet-950/40 text-violet-100 hover:bg-violet-950/60`}
              >
                Сохранить доску и bypass
              </button>
              {boardMsg ? (
                <span className="ml-2 text-xs text-slate-400">{boardMsg}</span>
              ) : null}
            </div>
          </div>
        ) : (
          <p className="mt-2 text-xs text-slate-500">Загрузка…</p>
        )}
      </div>

      <div className="mt-6 border-t border-white/10 pt-5">
        <div className="flex flex-wrap items-center justify-between gap-2">
          <h3 className="text-xs font-medium uppercase tracking-wide text-slate-500">
            settings.json (расширенный)
          </h3>
          <button
            type="button"
            onClick={() => void loadSettings()}
            disabled={busy}
            className={`${btnBase} border border-white/10 bg-black/30 px-3 py-1.5 text-xs text-slate-300`}
          >
            Перечитать
          </button>
        </div>
        <textarea
          className="mt-2 min-h-[200px] w-full rounded-xl border border-white/10 bg-black/40 p-3 font-mono text-[11px] leading-relaxed text-slate-200 focus:border-red-600 focus:outline-none focus:ring-2 focus:ring-red-600/35"
          value={settingsText}
          onChange={(e) => setSettingsText(e.target.value)}
          spellCheck={false}
        />
        <div className="mt-2 flex flex-wrap items-center gap-2">
          <button
            type="button"
            onClick={() => void onSaveSettings()}
            disabled={busy}
            className={`${btnBase} border border-red-600/50 bg-red-950/40 text-red-100 hover:bg-red-950/60`}
          >
            Сохранить
          </button>
          {settingsMsg ? (
            <span className="text-xs text-slate-400">{settingsMsg}</span>
          ) : null}
        </div>
      </div>

      {logsOpen ? (
        <div
          className="fixed inset-0 z-50 flex items-center justify-center bg-black/70 p-4 backdrop-blur-sm"
          role="dialog"
          aria-modal="true"
          aria-labelledby="proxy-logs-title"
          onClick={(e) => {
            if (e.target === e.currentTarget) setLogsOpen(false);
          }}
        >
          <div
            className="max-h-[85vh] w-full max-w-4xl overflow-hidden rounded-2xl border border-white/15 bg-[#0a0908] shadow-2xl"
            onClick={(e) => e.stopPropagation()}
          >
            <div className="flex items-center justify-between gap-3 border-b border-white/10 px-4 py-3">
              <h3 id="proxy-logs-title" className="text-sm font-semibold text-slate-100">
                Логи прокси (CONNECT)
              </h3>
              <div className="flex items-center gap-2">
                <button
                  type="button"
                  onClick={() => void onClearProxyLogs()}
                  className={`${btnBase} border border-white/15 bg-black/40 px-3 py-1.5 text-xs text-slate-300`}
                >
                  Очистить
                </button>
                <button
                  type="button"
                  onClick={() => setLogsOpen(false)}
                  className="rounded-lg p-2 text-slate-400 hover:bg-white/10 hover:text-slate-100"
                  aria-label="Закрыть"
                >
                  <X className="h-5 w-5" />
                </button>
              </div>
            </div>
            <p className="border-b border-white/5 px-4 py-2 text-[11px] text-slate-500">
              Формат: время · локальный пир · цель CONNECT · решение правил · маршрут · OK/FAIL
            </p>
            <pre className="max-h-[60vh] overflow-auto p-4 font-mono text-[11px] leading-relaxed text-slate-200">
              {proxyLogs.length === 0 ? (
                <span className="text-slate-500">
                  Нет записей. Запустите прокси и сгенерируйте трафик.
                </span>
              ) : (
                proxyLogs.map((e, i) => (
                  <div
                    key={`${e.timestampMs}-${i}-${e.clientAddr}`}
                    className="whitespace-pre-wrap break-all border-b border-white/5 py-1 last:border-0"
                  >
                    {formatProxyLogLine(e)}
                  </div>
                ))
              )}
            </pre>
          </div>
        </div>
      ) : null}
    </section>
  );
}
