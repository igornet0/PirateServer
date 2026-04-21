/**
 * control-api: список проектов на хосте и статус деплоя (JWT).
 */
import { invoke } from "@tauri-apps/api/core";
import {
  AlertCircle,
  Copy,
  FileCode,
  Loader2,
  LogIn,
  LogOut,
  Play,
  RefreshCw,
  RotateCcw,
  Server,
  Square,
  Terminal,
  Trash2,
  X,
} from "lucide-react";
import React, { useCallback, useEffect, useState } from "react";
import { createPortal } from "react-dom";
import { useI18n } from "./i18n";
import { CopyablePre } from "./ui/CopyablePre";
import { ModalDialog } from "./ui/ModalDialog";

const btnSm =
  "inline-flex items-center justify-center gap-1.5 rounded-lg px-2.5 py-1.5 text-xs font-semibold transition focus-visible:outline-none focus-visible:ring-2 focus-visible:ring-red-600/80 disabled:pointer-events-none disabled:opacity-50";

function controlApiAuthLostMessage(msg: string): boolean {
  const m = msg.toLowerCase();
  return (
    m.includes("not logged in") ||
    m.includes("sign in again") ||
    m.includes("401") ||
    m.includes("session expired")
  );
}

function isNetworkTimeoutLike(msg: string): boolean {
  const m = msg.toLowerCase();
  return (
    m.includes("timed out") ||
    m.includes("timeout") ||
    m.includes("operation timed out") ||
    m.includes("connect error")
  );
}

function sleep(ms: number): Promise<void> {
  return new Promise((resolve) => window.setTimeout(resolve, ms));
}

export type ServerProjectRow = {
  id: string;
  deployRoot: string;
  state: string;
  currentVersion: string;
  source: string;
  /** deploy-server max artifact bytes (GetStatus) when available */
  maxUploadBytes?: number | null;
  statusError?: string;
};

export type ServerProjectsOverviewData = {
  projects: ServerProjectRow[];
  error?: string;
};

type HostLogLine = {
  ts_ms: number;
  level: string;
  message: string;
};

type ProjectNginxSnippet = {
  path: string;
  configured: boolean;
  status?: string | null;
  reason_code?: string | null;
  hint?: string | null;
  content?: string | null;
};

function localizedNginxAbsentHint(
  language: string,
  reasonCode: string | null | undefined,
  serverHint: string | null | undefined,
): string {
  const rc = reasonCode ?? "";
  if (language === "ru") {
    switch (rc) {
      case "not_nginx_edge":
        return "Манифест не нацелен на nginx как edge-прокси (или при proxy.enabled указан другой backend).";
      case "no_upstream_routes":
        return "Нет upstream: задайте [proxy].routes или [services].web/api, либо [proxy].port / [health].port.";
      case "no_active_version":
        return "Нет активной версии релиза — сначала выполните деплой.";
      case "no_manifest_in_release":
        return "В каталоге релиза нет pirate.toml — задеплойте с корректным манифестом.";
      case "not_generated":
        return "Сниппет ожидался, но файл отсутствует; проверьте логи deploy-server или задеплойте снова.";
      default:
        break;
    }
  }
  return (serverHint && serverHint.trim()) || "";
}

type ProjectTelemetry = {
  project_id: string;
  state: string;
  pid?: number | null;
  cpu_percent?: number | null;
  ram_used_bytes?: number | null;
  ram_percent?: number | null;
  gpu_percent?: number | null;
  telemetry_available: boolean;
  logs_available: boolean;
  logs_tail: HostLogLine[];
  collected_at_ms: number;
  project_nginx?: ProjectNginxSnippet;
};

function formatBytes(n: number): string {
  if (!Number.isFinite(n) || n <= 0) return "—";
  const u = ["B", "KiB", "MiB", "GiB", "TiB"];
  let v = n;
  let i = 0;
  while (v >= 1024 && i < u.length - 1) {
    v /= 1024;
    i += 1;
  }
  return `${v.toFixed(v >= 10 || i === 0 ? 0 : 1)} ${u[i]}`;
}

function pathBasename(p: string): string {
  const t = p.trim();
  if (!t) return "";
  const parts = t.split(/[/\\]+/).filter(Boolean);
  return parts[parts.length - 1] ?? "";
}

/** Человекочитаемое имя в списке и в шапке модалки (не сырой `p-…` на всю ширину). */
function serverProjectDisplayTitle(
  row: ServerProjectRow,
  tr: (ru: string, en: string) => string,
): string {
  const root = row.deployRoot.trim();
  if (row.id === "default" || root.endsWith("/pirate/deploy")) {
    return tr("Клиентский проект local-stack", "local-stack client project");
  }
  const bn = pathBasename(root);
  if (bn && bn.length > 0 && bn !== row.id && bn.length < 80) {
    return bn;
  }
  if (row.id.length > 32) {
    return `${row.id.slice(0, 14)}…${row.id.slice(-10)}`;
  }
  return row.id;
}

function logLineRowClass(level: string): string {
  const l = (level || "info").toLowerCase();
  if (l === "error" || l === "err" || l === "fatal") {
    return "border-l-2 border-rose-500/50 text-rose-200/95";
  }
  if (l === "warn" || l === "warning") {
    return "border-l-2 border-amber-500/40 text-amber-200/90";
  }
  if (l === "debug" || l === "trace") {
    return "border-l-2 border-transparent text-slate-500";
  }
  return "border-l-2 border-transparent text-slate-200";
}

export function ServerProjectsOverview({
  grpcEndpoint,
  controlApiBase,
  serverControlApiPublic,
  serverControlApiDirect,
  modalPortalEl,
  onOpenConnectionSettings,
  onOpenProjectDeploy,
}: {
  /** Сохранённый deploy-server (gRPC) — основная связь с сервером. */
  grpcEndpoint: string | null;
  /** HTTP control-api из настроек на вкладке «Соединение» (для REST списка проектов). */
  controlApiBase: string;
  /** Из GetStatus.deploy (публичный URL за nginx). */
  serverControlApiPublic?: string | null;
  /** Из GetStatus — прямой bind :8080 и т.д. */
  serverControlApiDirect?: string | null;
  /** Элемент колонки workspace (родитель `relative`) — модалка порталится сюда, затемняется только правая часть, не сайдбар. */
  modalPortalEl?: HTMLElement | null;
  onOpenConnectionSettings: () => void;
  /** Перейти к вкладке «Проекты», чтобы настроить proxy/nginx в мастере деплоя. */
  onOpenProjectDeploy?: () => void;
}) {
  const { language, t } = useI18n();
  const tr = (ru: string, en: string) => (language === "ru" ? ru : en);
  const [username, setUsername] = useState("");
  const [password, setPassword] = useState("");
  const [sessionActive, setSessionActive] = useState(false);
  const [overview, setOverview] = useState<ServerProjectsOverviewData | null>(null);
  const [loading, setLoading] = useState(false);
  const [err, setErr] = useState<string | null>(null);
  const [loginMsg, setLoginMsg] = useState<string | null>(null);
  const [selectedProject, setSelectedProject] = useState<ServerProjectRow | null>(null);
  const [projectActionLoading, setProjectActionLoading] = useState(false);
  const [projectActionErr, setProjectActionErr] = useState<string | null>(null);
  const [projectLogs, setProjectLogs] = useState<HostLogLine[]>([]);
  const [projectLogsLoading, setProjectLogsLoading] = useState(false);
  const [projectLogsClearing, setProjectLogsClearing] = useState(false);
  const [logsAutoRefresh, setLogsAutoRefresh] = useState(false);
  const [projectMetrics, setProjectMetrics] = useState<ProjectTelemetry | null>(null);

  useEffect(() => {
    void (async () => {
      try {
        const active = await invoke<boolean>("control_api_session_active");
        setSessionActive(active);
      } catch {
        setSessionActive(false);
      }
    })();
  }, [controlApiBase]);

  const refresh = useCallback(async () => {
    setLoading(true);
    setErr(null);
    try {
      const o = await invoke<ServerProjectsOverviewData>("fetch_server_projects_overview");
      setOverview(o);
      setSessionActive(true);
    } catch (e) {
      const msg = String(e);
      setErr(msg);
      setOverview(null);
      if (controlApiAuthLostMessage(msg)) {
        setSessionActive(false);
      }
    } finally {
      setLoading(false);
    }
  }, []);

  const onLogin = async () => {
    setLoginMsg(null);
    setErr(null);
    const base = controlApiBase.trim();
    if (!base) {
      setLoginMsg(
        tr(
          "Задайте HTTP-адрес control-api на вкладке «Соединение» (рядом с gRPC) и сохраните.",
          "Set control-api HTTP address in the Connection tab (next to gRPC) and save it.",
        ),
      );
      return;
    }
    const u = username.trim();
    const p = password.trim();
    if (!u || !p) {
      setLoginMsg(t("auto.ServerProjectsOverview_tsx.1"));
      return;
    }
    setLoading(true);
    try {
      let lastErr: unknown = null;
      for (let i = 0; i < 3; i += 1) {
        try {
          await invoke("control_api_login", { baseUrl: base, username: u, password: p });
          lastErr = null;
          break;
        } catch (e) {
          lastErr = e;
          if (!isNetworkTimeoutLike(String(e)) || i === 2) break;
          await sleep(450 * (i + 1));
        }
      }
      if (lastErr) throw lastErr;
      setPassword("");
      setUsername("");
      setSessionActive(true);
      setLoginMsg(null);
      await refresh();
    } catch (e) {
      const raw = String(e);
      let probe = "";
      let restartHint = false;
      try {
        probe = await invoke<string>("control_api_health_probe", { baseUrl: base });
      } catch (probeErr) {
        probe = `health_probe_error=${String(probeErr)}`;
      }
      try {
        restartHint = await invoke<boolean>("control_api_recent_restart_hint");
      } catch {
        restartHint = false;
      }
      const suffix = ` (base=${base}; ${probe}; restart_recent=${restartHint})`;
      setLoginMsg(raw.includes("(base=") ? raw : `${raw}${suffix}`);
    } finally {
      setLoading(false);
    }
  };

  const onLogout = async () => {
    setErr(null);
    setLoginMsg(null);
    try {
      await invoke("control_api_logout");
      setSessionActive(false);
      setOverview(null);
    } catch (e) {
      setErr(String(e));
    }
  };

  const openProjectModal = useCallback(
    async (row: ServerProjectRow) => {
      setSelectedProject(row);
      setProjectActionErr(null);
      setProjectLogs([]);
      setProjectMetrics(null);
      setLogsAutoRefresh(false);
      setProjectLogsLoading(true);

      const loadTelemetry = async () => {
        try {
          const raw = await invoke<string>("control_api_fetch_project_telemetry_json", {
            projectId: row.id,
            logsLimit: 120,
          });
          const parsed = JSON.parse(raw) as ProjectTelemetry;
          setProjectMetrics(parsed);
          setProjectLogs(Array.isArray(parsed.logs_tail) ? parsed.logs_tail : []);
        } catch (e) {
          setProjectActionErr(String(e));
        } finally {
          setProjectLogsLoading(false);
        }
      };

      await loadTelemetry();
    },
    [],
  );

  const refreshProjectTelemetry = useCallback(
    async (opts?: { silent?: boolean }) => {
      if (!selectedProject) return;
      const silent = Boolean(opts?.silent);
      if (!silent) setProjectLogsLoading(true);
      setProjectActionErr(null);
      try {
        const raw = await invoke<string>("control_api_fetch_project_telemetry_json", {
          projectId: selectedProject.id,
          logsLimit: 120,
        });
        const parsed = JSON.parse(raw) as ProjectTelemetry;
        setProjectMetrics(parsed);
        setProjectLogs(Array.isArray(parsed.logs_tail) ? parsed.logs_tail : []);
      } catch (e) {
        setProjectActionErr(String(e));
      } finally {
        if (!silent) setProjectLogsLoading(false);
      }
    },
    [selectedProject],
  );

  const runProjectAction = useCallback(
    async (action: "start" | "stop" | "restart") => {
      if (!selectedProject) return;
      setProjectActionErr(null);
      setProjectActionLoading(true);
      try {
        if (action === "stop") {
          await invoke("control_api_stop_process_json", { projectId: selectedProject.id });
        } else {
          // control-api does not expose a dedicated "start" endpoint;
          // restart starts the process if it is currently stopped.
          await invoke("control_api_restart_process_json", { projectId: selectedProject.id });
        }
        await refresh();
        await refreshProjectTelemetry();
      } catch (e) {
        setProjectActionErr(String(e));
      } finally {
        setProjectActionLoading(false);
      }
    },
    [refresh, selectedProject, refreshProjectTelemetry],
  );

  useEffect(() => {
    if (!selectedProject || !logsAutoRefresh) return;
    const id = window.setInterval(() => {
      void refreshProjectTelemetry({ silent: true });
    }, 3000);
    return () => window.clearInterval(id);
  }, [selectedProject, logsAutoRefresh, refreshProjectTelemetry]);

  const clearProjectRuntimeLog = useCallback(async () => {
    if (!selectedProject) return;
    setProjectActionErr(null);
    setProjectLogsClearing(true);
    try {
      await invoke<string>("control_api_clear_project_runtime_log", {
        projectId: selectedProject.id,
      });
      await refreshProjectTelemetry();
    } catch (e) {
      setProjectActionErr(String(e));
    } finally {
      setProjectLogsClearing(false);
    }
  }, [selectedProject, refreshProjectTelemetry]);

  const projectModal =
    selectedProject != null ? (
      <ModalDialog
        open
        overlay={modalPortalEl ? "container" : "viewport"}
        zClassName="z-modalNested"
        onClose={() => {
          setLogsAutoRefresh(false);
          setSelectedProject(null);
        }}
        panelClassName="w-full max-w-3xl max-h-[92vh] min-h-0"
        aria-labelledby="server-project-modal-title"
      >
          <div className="flex max-h-[92vh] min-h-0 flex-col overflow-hidden rounded-2xl border border-white/15 bg-surface shadow-2xl">
            <div className="shrink-0 border-b border-white/10 bg-black/20 px-4 py-4 sm:px-5">
              <div className="flex items-start gap-3">
                <div className="min-w-0 flex-1">
                  <p className="text-[10px] font-semibold uppercase tracking-wider text-red-400/70">
                    {tr("Проект на сервере", "Server project")}
                  </p>
                  <h3
                    id="server-project-modal-title"
                    className="mt-1 flex items-start gap-2 text-lg font-semibold leading-snug text-slate-100"
                  >
                    <Terminal className="mt-0.5 h-5 w-5 shrink-0 text-amber-200/85" aria-hidden />
                    <span className="min-w-0">
                      {serverProjectDisplayTitle(selectedProject, tr)}
                    </span>
                  </h3>
                  <div className="mt-2 flex flex-wrap items-center gap-2">
                    <span
                      className={`inline-flex items-center rounded-full px-2.5 py-0.5 text-[11px] font-medium ring-1 ${
                        selectedProject.state?.toLowerCase() === "running"
                          ? "bg-emerald-500/15 text-emerald-200 ring-emerald-500/35"
                          : selectedProject.state?.toLowerCase() === "error"
                            ? "bg-rose-500/15 text-rose-200 ring-rose-500/35"
                            : "bg-slate-600/25 text-slate-300 ring-slate-500/30"
                      }`}
                    >
                      {selectedProject.state}
                    </span>
                    <span className="text-[11px] text-slate-500">
                      {tr("Релиз", "Release")}:{" "}
                      <span className="font-mono text-slate-300">{selectedProject.currentVersion || "—"}</span>
                    </span>
                  </div>
                  <div className="mt-3 space-y-2">
                    <div className="flex items-start justify-between gap-2 rounded-lg border border-white/10 bg-black/30 px-2.5 py-1.5">
                      <div className="min-w-0">
                        <p className="text-[10px] font-medium uppercase tracking-wide text-slate-500">
                          ID
                        </p>
                        <p className="break-all font-mono text-[11px] text-orange-200/85">{selectedProject.id}</p>
                      </div>
                      <button
                        type="button"
                        data-modal-initial-focus
                        onClick={() => {
                          void navigator.clipboard.writeText(selectedProject.id);
                        }}
                        className="shrink-0 rounded-lg border border-white/10 bg-white/5 p-2 text-slate-400 transition-colors duration-150 hover:bg-white/10 hover:text-slate-200 focus-visible:outline-none focus-visible:ring-2 focus-visible:ring-red-600/70"
                        title={tr("Копировать ID", "Copy ID")}
                        aria-label={tr("Копировать ID", "Copy ID")}
                      >
                        <Copy className="h-4 w-4" />
                      </button>
                    </div>
                    <div className="flex items-start justify-between gap-2 rounded-lg border border-white/10 bg-black/30 px-2.5 py-1.5">
                      <div className="min-w-0">
                        <p className="text-[10px] font-medium uppercase tracking-wide text-slate-500">
                          {tr("Каталог на сервере", "Deploy root")}
                        </p>
                        <p className="break-all font-mono text-[11px] text-slate-400">{selectedProject.deployRoot}</p>
                      </div>
                      <button
                        type="button"
                        onClick={() => {
                          void navigator.clipboard.writeText(selectedProject.deployRoot);
                        }}
                        className="shrink-0 rounded-lg border border-white/10 bg-white/5 p-2 text-slate-400 transition-colors duration-150 hover:bg-white/10 hover:text-slate-200 focus-visible:outline-none focus-visible:ring-2 focus-visible:ring-red-600/70"
                        title={tr("Копировать путь", "Copy path")}
                        aria-label={tr("Копировать путь", "Copy path")}
                      >
                        <Copy className="h-4 w-4" />
                      </button>
                    </div>
                    {selectedProject.maxUploadBytes != null && selectedProject.maxUploadBytes > 0 ? (
                      <div className="rounded-lg border border-white/10 bg-black/30 px-2.5 py-1.5">
                        <p className="text-[10px] font-medium uppercase tracking-wide text-slate-500">
                          {tr("Лимит загрузки артефакта", "Max artifact upload")}
                        </p>
                        <p className="font-mono text-[11px] text-slate-400">
                          {selectedProject.maxUploadBytes.toLocaleString()} {tr("байт", "bytes")}
                        </p>
                      </div>
                    ) : null}
                  </div>
                </div>
                <button
                  type="button"
                  className="shrink-0 rounded-xl border border-white/10 bg-white/5 p-2.5 text-slate-400 transition-colors duration-150 hover:bg-white/10 hover:text-slate-100 focus-visible:outline-none focus-visible:ring-2 focus-visible:ring-red-600/70"
                  onClick={() => setSelectedProject(null)}
                  aria-label={tr("Закрыть", "Close")}
                >
                  <X className="h-5 w-5" />
                </button>
              </div>

              <div className="mt-4 flex flex-wrap items-center justify-between gap-2 border-t border-white/5 pt-3">
                <div className="flex flex-wrap gap-2">
                  <button
                    type="button"
                    disabled={projectActionLoading || projectLogsLoading}
                    onClick={() => void runProjectAction("start")}
                    className={`${btnSm} border border-emerald-700/40 bg-emerald-900/25 text-emerald-100 hover:bg-emerald-800/30`}
                  >
                    {projectActionLoading ? <Loader2 className="h-3.5 w-3.5 animate-spin" /> : <Play className="h-3.5 w-3.5" />}
                    {tr("Запустить", "Start")}
                  </button>
                  <button
                    type="button"
                    disabled={projectActionLoading || projectLogsLoading}
                    onClick={() => void runProjectAction("stop")}
                    className={`${btnSm} border border-red-800/40 bg-red-900/25 text-red-100 hover:bg-red-800/30`}
                  >
                    {projectActionLoading ? <Loader2 className="h-3.5 w-3.5 animate-spin" /> : <Square className="h-3.5 w-3.5" />}
                    {tr("Остановить", "Stop")}
                  </button>
                  <button
                    type="button"
                    disabled={projectActionLoading || projectLogsLoading}
                    onClick={() => void runProjectAction("restart")}
                    className={`${btnSm} border border-amber-700/40 bg-amber-900/25 text-amber-100 hover:bg-amber-800/30`}
                  >
                    {projectActionLoading ? (
                      <Loader2 className="h-3.5 w-3.5 animate-spin" />
                    ) : (
                      <RotateCcw className="h-3.5 w-3.5" />
                    )}
                    {tr("Перезапустить", "Restart")}
                  </button>
                </div>
                <button
                  type="button"
                  disabled={projectLogsLoading || projectActionLoading}
                  onClick={() => void refreshProjectTelemetry()}
                  className={`${btnSm} border border-white/15 bg-white/5 text-slate-200 hover:bg-white/10`}
                >
                  {projectLogsLoading ? (
                    <Loader2 className="h-3.5 w-3.5 animate-spin" />
                  ) : (
                    <RefreshCw className="h-3.5 w-3.5" />
                  )}
                  {tr("Обновить данные", "Refresh")}
                </button>
              </div>
            </div>

            <div className="min-h-0 flex-1 overflow-y-auto px-4 py-4 sm:px-5">
              <div className="rounded-xl border border-white/10 bg-black/25 p-3 text-xs text-slate-400">
                <div className="font-semibold text-slate-200">{tr("Потребление ресурсов", "Resource usage")}</div>
                {projectMetrics?.pid != null ? (
                  <p className="mt-1 text-[11px] text-slate-500">
                    PID: <span className="font-mono text-slate-400">{projectMetrics.pid}</span>
                  </p>
                ) : null}
                <div className="mt-2 grid gap-2 sm:grid-cols-3">
                  <div>CPU: {projectMetrics?.cpu_percent != null ? `${projectMetrics.cpu_percent.toFixed(1)}%` : "—"}</div>
                  <div>
                    RAM:{" "}
                    {projectMetrics?.ram_used_bytes != null
                      ? `${formatBytes(projectMetrics.ram_used_bytes)}${
                          projectMetrics.ram_percent != null ? ` (${projectMetrics.ram_percent.toFixed(1)}%)` : ""
                        }`
                      : "—"}
                  </div>
                  <div>GPU: {projectMetrics?.gpu_percent != null ? `${projectMetrics.gpu_percent.toFixed(1)}%` : "—"}</div>
                </div>
                {!projectMetrics?.telemetry_available ? (
                  <p className="mt-2 text-[11px] text-slate-500">
                    {tr(
                      "Метрики проекта пока недоступны (процесс не запущен или PID не найден).",
                      "Project metrics are currently unavailable (process is not running or PID is missing).",
                    )}
                  </p>
                ) : null}
              </div>

              <details className="mt-4 rounded-xl border border-white/10 bg-black/30 p-3" open>
                <summary className="cursor-pointer list-none text-xs font-semibold uppercase tracking-wide text-slate-400 [&::-webkit-details-marker]:hidden">
                  <span className="inline-flex items-center gap-2">
                    <FileCode className="h-3.5 w-3.5 text-emerald-300/80" />
                    {tr("Nginx проекта (релиз)", "Project nginx (release)")}
                  </span>
                </summary>
                <p className="mt-2 text-[11px] leading-relaxed text-slate-500">
                  {tr(
                    "Файл: releases/<version>/pirate-nginx-snippet.conf — генерируется при деплое, когда манифест задаёт nginx edge и есть upstream (маршруты, services.web/api или порт proxy/health).",
                    "File: releases/<version>/pirate-nginx-snippet.conf — generated on deploy when the manifest targets nginx as the edge proxy and upstreams exist (routes, services.web/api, or proxy/health port).",
                  )}
                </p>
                {projectMetrics?.project_nginx?.configured && projectMetrics.project_nginx.content ? (
                  <>
                    <p className="mb-2 font-mono text-[11px] text-slate-500">{projectMetrics.project_nginx.path}</p>
                    <CopyablePre
                      value={projectMetrics.project_nginx.content}
                      placeholder="—"
                      className="rounded-lg border border-white/10 bg-black/40 p-3 font-mono text-[11px] leading-relaxed text-slate-200"
                      maxHeightClass="max-h-56"
                    />
                  </>
                ) : (
                  <div className="mt-2 rounded-lg border border-amber-900/35 bg-amber-950/20 p-3 text-[11px] leading-relaxed text-amber-100/90">
                    {(() => {
                      const absentHint = localizedNginxAbsentHint(
                        language,
                        projectMetrics?.project_nginx?.reason_code,
                        projectMetrics?.project_nginx?.hint,
                      );
                      return absentHint ? (
                        <p className="mb-2 text-amber-50/95">{absentHint}</p>
                      ) : null;
                    })()}
                    <p>
                      {tr(
                        "Сниппет отсутствует: проверьте [proxy] и upstream в pirate.toml, выполните деплой (или «Apply gen» локально, затем снова деплой).",
                        "Snippet missing: check [proxy] and upstreams in pirate.toml, deploy (or run “Apply gen” locally, then deploy again).",
                      )}
                    </p>
                    {onOpenProjectDeploy ? (
                      <button
                        type="button"
                        onClick={() => {
                          onOpenProjectDeploy();
                        }}
                        className={`${btnSm} mt-3 border border-amber-700/50 bg-amber-950/40 text-amber-100 hover:bg-amber-900/50`}
                      >
                        {tr("Открыть вкладку «Проекты»", "Open Projects tab")}
                      </button>
                    ) : null}
                  </div>
                )}
              </details>

              <details className="mt-4 rounded-xl border border-white/10 bg-black/30 p-3" open>
                <summary className="cursor-pointer list-none text-xs font-semibold uppercase tracking-wide text-slate-400 [&::-webkit-details-marker]:hidden">
                  {tr("Логи с сервера (хвост)", "Server logs (tail)")}
                </summary>
                <div className="mt-3 flex flex-wrap items-center gap-3 border-b border-white/5 pb-3">
                  <label className="inline-flex cursor-pointer items-center gap-2 text-[11px] text-slate-400">
                    <input
                      type="checkbox"
                      className="rounded border-white/20 bg-black/40"
                      checked={logsAutoRefresh}
                      onChange={(e) => setLogsAutoRefresh(e.target.checked)}
                    />
                    {tr("Автообновление каждые 3 с", "Auto-refresh every 3s")}
                  </label>
                  <button
                    type="button"
                    disabled={projectLogsClearing || projectLogsLoading}
                    onClick={() => void clearProjectRuntimeLog()}
                    className={`${btnSm} border border-white/15 bg-white/5 text-slate-200 hover:bg-white/10 disabled:opacity-50`}
                  >
                    {projectLogsClearing ? (
                      <Loader2 className="h-3.5 w-3.5 animate-spin" />
                    ) : (
                      <Trash2 className="h-3.5 w-3.5" />
                    )}
                    {tr("Очистить логи", "Clear logs")}
                  </button>
                </div>
                <p className="mt-2 text-[11px] leading-relaxed text-slate-600">
                  {tr(
                    "Источник на хосте: каталог деплоя проекта/.pirate/runtime.log (только этот файл; Docker/другие рантаймы могут не писать сюда).",
                    "Host source: <project deploy root>/.pirate/runtime.log (this file only; Docker and other runtimes may not write here).",
                  )}
                </p>
                {projectLogsLoading ? (
                  <div className="py-6 text-center text-slate-500">
                    <Loader2 className="mx-auto h-5 w-5 animate-spin opacity-70" />
                  </div>
                ) : projectLogs.length ? (
                  <div className="mt-2 max-h-72 overflow-auto rounded-lg border border-white/10 bg-black/40 p-3 font-mono text-[11px] leading-relaxed">
                    {projectLogs.map((line, i) => (
                      <div
                        key={`${line.ts_ms}-${i}`}
                        className={`py-0.5 pl-2 ${logLineRowClass(line.level)}`}
                      >
                        <span className="text-slate-500">[{line.level || "info"}]</span> {line.message}
                      </div>
                    ))}
                  </div>
                ) : (
                  <p className="mt-2 text-xs text-slate-500">
                    {projectMetrics?.logs_available === false
                      ? tr(
                          "Логи проекта недоступны (нет runtime.log или источник логов не настроен).",
                          "Project logs are unavailable (runtime.log is missing or log source is not configured).",
                        )
                      : tr("Логи недоступны.", "Logs are unavailable.")}
                  </p>
                )}
              </details>

              {projectActionErr ? (
                <p className="mt-4 rounded-lg border border-red-900/40 bg-red-950/30 px-3 py-2 text-xs text-red-200/90">
                  {projectActionErr}
                </p>
              ) : null}
            </div>
          </div>
        </ModalDialog>
    )
    : null;

  return (
    <section
      className="rounded-2xl border border-white/10 bg-surface/90 p-4 shadow-card backdrop-blur"
      aria-labelledby="server-projects-heading"
    >
      <div className="flex flex-wrap items-start justify-between gap-3">
        <div>
          <h2
            id="server-projects-heading"
            className="flex items-center gap-2 text-sm font-semibold text-slate-100"
          >
            <Server className="h-4 w-4 text-red-400/85" aria-hidden />
            {t("auto.ServerProjectsOverview_tsx.2")}
          </h2>
          <p className="mt-1 text-xs text-slate-500">
            {tr(
              "Связь с сервером — через gRPC (deploy-server). Список проектов и статус подгружаются по HTTP control-api с того же хоста; логин дашборда нужен для JWT.",
              "Server connection goes through gRPC (deploy-server). Project list and status are loaded via HTTP control-api from the same host; dashboard login is required for JWT.",
            )}
          </p>
        </div>
        <div className="flex flex-wrap gap-2">
          <button
            type="button"
            disabled={loading}
            onClick={() => void refresh()}
            className={`${btnSm} border border-white/15 bg-white/5 hover:bg-white/10`}
          >
            {loading ? <Loader2 className="h-3.5 w-3.5 animate-spin" /> : <RefreshCw className="h-3.5 w-3.5" />}
            {t("auto.ServerProjectsOverview_tsx.3")}
          </button>
          {sessionActive ? (
            <button
              type="button"
              onClick={() => void onLogout()}
              className={`${btnSm} border border-white/10 text-slate-300 hover:bg-white/10`}
            >
              <LogOut className="h-3.5 w-3.5" />
              {t("auto.ServerProjectsOverview_tsx.4")}
            </button>
          ) : null}
        </div>
      </div>

      <div className="mt-4 space-y-3 rounded-xl border border-white/10 bg-black/20 p-3">
        <div className="text-xs text-slate-500">
          <span className="font-medium text-slate-400">gRPC: </span>
          {grpcEndpoint ? (
            <code className="break-all rounded bg-black/40 px-1.5 py-0.5 font-mono text-[11px] text-amber-200/85">
              {grpcEndpoint}
            </code>
          ) : (
            <span className="text-rose-300/90">
              {t("auto.ServerProjectsOverview_tsx.5")}
            </span>
          )}
        </div>
        <div className="text-xs text-slate-500">
          <span className="font-medium text-slate-400">
            {t("auto.ServerProjectsOverview_tsx.6")}
          </span>
          {controlApiBase.trim() ? (
            <code className="break-all rounded bg-black/40 px-1.5 py-0.5 font-mono text-[11px] text-slate-400">
              {controlApiBase.trim()}
            </code>
          ) : (
            <span>
              {t("auto.ServerProjectsOverview_tsx.7")}{" "}
              <button
                type="button"
                onClick={onOpenConnectionSettings}
                className="text-orange-200/90 underline decoration-amber-600/50 underline-offset-2 hover:text-amber-100"
              >
                {t("auto.ServerProjectsOverview_tsx.8")}
              </button>
            </span>
          )}
        </div>
        {serverControlApiPublic || serverControlApiDirect ? (
          <div className="text-xs text-slate-500">
            <span className="font-medium text-slate-400">{t("auto.ServerProjectsOverview_tsx.9")}</span>
            {serverControlApiPublic ? (
              <span className="mr-2">
                {t("auto.ServerProjectsOverview_tsx.10")}{" "}
                <code className="break-all rounded bg-black/40 px-1.5 py-0.5 font-mono text-[11px] text-emerald-200/70">
                  {serverControlApiPublic}
                </code>
              </span>
            ) : null}
            {serverControlApiDirect ? (
              <span>
                {t("auto.ServerProjectsOverview_tsx.11")}{" "}
                <code className="break-all rounded bg-black/40 px-1.5 py-0.5 font-mono text-[11px] text-slate-500">
                  {serverControlApiDirect}
                </code>
              </span>
            ) : null}
          </div>
        ) : null}
        {sessionActive ? (
          <div className="border-t border-white/5 pt-3">
            <p className="text-xs leading-relaxed text-slate-400">
              {tr(
                "Сессия control-api активна. Список проектов загружается по REST с сохранённым JWT.",
                "control-api session is active. Projects are loaded via REST using saved JWT.",
              )}
            </p>
          </div>
        ) : (
          <>
            <div className="flex flex-wrap items-end gap-2 border-t border-white/5 pt-3">
              <p className="w-full text-xs text-slate-500">
                {t("auto.ServerProjectsOverview_tsx.12")}
              </p>
              <label className="flex min-w-[8rem] flex-1 flex-col gap-1 text-xs text-slate-500">
                {t("auto.ServerProjectsOverview_tsx.13")}
                <input
                  type="text"
                  autoComplete="username"
                  value={username}
                  onChange={(e) => setUsername(e.target.value)}
                  className="rounded-lg border border-white/10 bg-black/30 px-2 py-1.5 text-sm text-slate-100"
                />
              </label>
              <label className="flex min-w-[8rem] flex-1 flex-col gap-1 text-xs text-slate-500">
                {t("auto.ServerProjectsOverview_tsx.14")}
                <input
                  type="password"
                  autoComplete="current-password"
                  value={password}
                  onChange={(e) => setPassword(e.target.value)}
                  className="rounded-lg border border-white/10 bg-black/30 px-2 py-1.5 text-sm text-slate-100"
                />
              </label>
              <button
                type="button"
                disabled={loading}
                onClick={() => void onLogin()}
                className={`${btnSm} border border-red-800/40 bg-red-950/35 text-amber-100 hover:bg-red-950/55`}
              >
                <LogIn className="h-3.5 w-3.5" />
                {t("auto.ServerProjectsOverview_tsx.15")}
              </button>
            </div>
            {loginMsg ? <p className="text-xs text-slate-400">{loginMsg}</p> : null}
          </>
        )}
      </div>

      <p className="mt-3 text-xs leading-relaxed text-slate-500">
        {tr(
          "CPU, RAM и GPU по отдельным проектам в этой версии не приходят с сервера; общие метрики хоста — в блоке «Метрики хоста» ниже. Трансляция логов и уровни из pirate.toml пока не отображаются (потребуется расширение API и манифеста).",
          "Per-project CPU, RAM, and GPU are not provided by the server in this version; host-wide metrics are shown in the Host Metrics section below. Log streams and levels from pirate.toml are not displayed yet (requires API and manifest extensions).",
        )}
      </p>

      {err ? (
        <p className="mt-3 flex items-start gap-2 rounded-lg border border-red-900/40 bg-red-950/30 px-3 py-2 text-sm text-red-200/90">
          <AlertCircle className="mt-0.5 h-4 w-4 shrink-0" />
          {err}
        </p>
      ) : null}

      <div className="mt-4 overflow-x-auto rounded-xl border border-white/10">
        <table className="w-full min-w-[20rem] text-left text-sm">
          <thead>
            <tr className="border-b border-white/10 bg-black/25 text-xs uppercase tracking-wide text-slate-500">
              <th className="px-3 py-2 font-medium">{t("auto.ServerProjectsOverview_tsx.16")}</th>
              <th className="px-3 py-2 font-medium">{t("auto.ServerProjectsOverview_tsx.17")}</th>
              <th className="px-3 py-2 font-medium">{t("auto.ServerProjectsOverview_tsx.18")}</th>
              <th className="px-3 py-2 font-medium">{t("auto.ServerProjectsOverview_tsx.19")}</th>
              <th className="px-3 py-2 font-medium">CPU / RAM / GPU</th>
            </tr>
          </thead>
          <tbody>
            {loading && !overview?.projects.length ? (
              <tr>
                <td colSpan={5} className="px-3 py-6 text-center text-slate-500">
                  <Loader2 className="mx-auto h-5 w-5 animate-spin opacity-70" />
                </td>
              </tr>
            ) : null}
            {!loading && !overview?.projects.length ? (
              <tr>
                <td colSpan={5} className="px-3 py-4 text-center text-xs text-slate-500">
                  {sessionActive
                    ? t("auto.ServerProjectsOverview_tsx.20")
                    : t("auto.ServerProjectsOverview_tsx.21")}
                </td>
              </tr>
            ) : null}
            {overview?.projects.map((row) => (
              <tr
                key={row.id}
                className="cursor-pointer border-b border-white/5 transition hover:bg-white/5 last:border-0"
                onClick={() => void openProjectModal(row)}
              >
                <td className="px-3 py-2">
                  <button
                    type="button"
                    className="block w-full text-left font-medium text-slate-200 underline decoration-dotted underline-offset-2 hover:text-amber-100"
                    onClick={(e) => {
                      e.stopPropagation();
                      void openProjectModal(row);
                    }}
                  >
                    <span className="block">{serverProjectDisplayTitle(row, tr)}</span>
                    <span
                      className="mt-0.5 block max-w-[18rem] truncate font-mono text-[10px] text-slate-500"
                      title={row.id}
                    >
                      {row.id}
                    </span>
                  </button>
                  <div className="max-w-[14rem] truncate font-mono text-[11px] text-slate-500" title={row.deployRoot}>
                    {row.deployRoot}
                  </div>
                  {row.statusError ? (
                    <div className="mt-1 text-[11px] text-rose-300/90">{row.statusError}</div>
                  ) : null}
                </td>
                <td className="px-3 py-2 text-slate-300">{row.state}</td>
                <td className="px-3 py-2 font-mono text-xs text-slate-400">{row.currentVersion}</td>
                <td className="px-3 py-2 text-xs text-slate-500">{row.source}</td>
                <td className="px-3 py-2 font-mono text-xs text-slate-600">
                  {tr("Открыть карточку", "Open details")}
                </td>
              </tr>
            ))}
          </tbody>
        </table>
      </div>

      {projectModal && modalPortalEl ? createPortal(projectModal, modalPortalEl) : projectModal}

    </section>
  );
}
