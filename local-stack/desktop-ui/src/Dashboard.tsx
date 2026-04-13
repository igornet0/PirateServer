/**
 * PirateClient desktop shell — главный экран.
 *
 * Интеграция с Tauri (вызываются из этого файла):
 * - `get_saved_grpc_endpoint` — сохранённый URL gRPC
 * - `get_control_api_base` / `set_control_api_base` — HTTP base control-api (графики series)
 * - `refresh_grpc_status` / `connect_grpc_bundle` / `clear_grpc_connection` — соединение
 * - `fetch_remote_host_stats` / `fetch_remote_host_stats_detail` — метрики хоста (gRPC)
 * - `list_server_bookmarks` / `add_server_bookmark` / `delete_server_bookmark` / `activate_server_bookmark` / `rename_server_bookmark`
 * - `pick_deploy_directory` / `set_active_project` / `deploy_from_directory`
 * - `pick_server_stack_tar_gz` / `apply_server_stack_update` / `fetch_server_stack_info_cmd` (OTA host bundle)
 * - `start_display_ingest` / `display_ingest_base` / `display_ingest_export_consumer_config` — display stream receive
 * - `get_display_stream_prefs` / `set_display_stream_prefs` — local stream send/receive flags
 * - `internet_proxy_start` / `internet_proxy_stop` / `internet_proxy_status` — локальный CONNECT-прокси
 * - `load_client_settings_json` / `save_client_settings_json` / `apply_default_rules_preset_cmd` — settings.json и пресеты правил
 *
 * Вкладки: «Обзор», «Соединение», «Интернет» (прокси и правила как у `pirate board`).
 *
 * Примеры будущих расширений (закомментируйте и подключите при необходимости):
 * // await invoke("get_metrics");
 * // await invoke("deploy_artifact", { ... });
 * // await invoke("change_endpoint", { url });
 */
import { invoke } from "@tauri-apps/api/core";
import { listen } from "@tauri-apps/api/event";
import {
  Activity,
  AlertCircle,
  Check,
  Copy,
  FileArchive,
  FolderOpen,
  LayoutDashboard,
  Link2,
  Loader2,
  Globe,
  Pencil,
  Plus,
  RefreshCw,
  Server,
  Trash2,
  X,
} from "lucide-react";
import React, { useCallback, useEffect, useMemo, useState } from "react";
import { DisplayStreamPanel } from "./DisplayStreamPanel";
import { InternetTrafficPanel } from "./InternetTrafficPanel";
import { HostMetricsPanel } from "./HostMetricsPanel";
import {
  MOCK_HOST_STATS,
  parseHostStatsSnapshot,
  type HostStatsSnapshot,
} from "./host-stats-types";

// -----------------------------------------------------------------------------
// Types & mock data (used when Tauri is unavailable or for Storybook-style preview)
// -----------------------------------------------------------------------------

type GrpcConnectResult = {
  endpoint: string;
  currentVersion: string;
  state: string;
};

type ServerBookmark = {
  id: string;
  label: string;
  url: string;
};

type DeployOutcome = {
  status: string;
  deployedVersion: string;
  artifactBytes: number;
  chunkCount: number;
};

type ServerStackOutcome = {
  status: string;
  appliedVersion: string;
  deployServerPkgVersion?: string | null;
  controlApiPkgVersion?: string | null;
};

/** @deprecated Use HostStatsSnapshot from `./host-stats-types` */
export type HostMetrics = HostStatsSnapshot;

function formatBytes(n: number): string {
  if (!Number.isFinite(n)) return "—";
  if (n >= 1e12) return `${(n / 1e12).toFixed(2)} TB`;
  if (n >= 1e9) return `${(n / 1e9).toFixed(2)} GB`;
  if (n >= 1e6) return `${(n / 1e6).toFixed(2)} MB`;
  if (n >= 1e3) return `${(n / 1e3).toFixed(2)} KB`;
  return `${Math.round(n)} B`;
}

/** Match Rust `normalize_url` / `normalize_endpoint` for bookmark vs active endpoint. */
function normalizeGrpcUrl(s: string): string {
  return s.trim().replace(/\/+$/, "");
}

// -----------------------------------------------------------------------------
// UI primitives
// -----------------------------------------------------------------------------

function ProgressBar({
  ratio,
  className = "",
}: {
  /** 0..1 */
  ratio: number;
  className?: string;
}) {
  const w = Math.min(100, Math.max(0, ratio * 100));
  return (
    <div
      className={`h-2.5 w-full overflow-hidden rounded-full bg-black/25 dark:bg-white/10 ${className}`}
    >
      <div
        className="h-full rounded-full bg-gradient-to-r from-red-700 via-amber-600 to-red-600 shadow-[0_0_14px_rgba(220,38,38,0.45)] transition-[width] duration-500 ease-out"
        style={{ width: `${w}%` }}
      />
    </div>
  );
}

const btnBase =
  "inline-flex items-center justify-center gap-2 rounded-xl px-4 py-2.5 text-sm font-semibold transition-all duration-200 focus-visible:outline-none focus-visible:ring-2 focus-visible:ring-red-600/80 focus-visible:ring-offset-2 focus-visible:ring-offset-[#050204] active:scale-[0.98] disabled:pointer-events-none disabled:opacity-50";

// -----------------------------------------------------------------------------
// Dashboard
// -----------------------------------------------------------------------------

type MainTab = "overview" | "connection" | "internet";

export function Dashboard() {
  const [mainTab, setMainTab] = useState<MainTab>("overview");

  const [endpoint, setEndpoint] = useState<string | null>(null);
  const [grpcLive, setGrpcLive] = useState<GrpcConnectResult | null>(null);
  const [grpcErr, setGrpcErr] = useState<string | null>(null);
  const [serverLoading, setServerLoading] = useState(false);

  const [metrics, setMetrics] = useState<HostStatsSnapshot | null>(null);
  const [metricsLoading, setMetricsLoading] = useState(false);
  const [metricsErr, setMetricsErr] = useState<string | null>(null);
  const [useMockMetrics, setUseMockMetrics] = useState(false);

  const [bookmarks, setBookmarks] = useState<ServerBookmark[]>([]);

  const [deployDir, setDeployDir] = useState<string | null>(null);
  const [deployVersion, setDeployVersion] = useState("v1.0.0");
  const [deployProject, setDeployProject] = useState("default");
  const [deploying, setDeploying] = useState(false);
  const [deployProgress, setDeployProgress] = useState(0);
  const [deployMsg, setDeployMsg] = useState<string | null>(null);
  const [deployCancelRequested, setDeployCancelRequested] = useState(false);

  const [stackPath, setStackPath] = useState<string | null>(null);
  const [stackVersion, setStackVersion] = useState("stack-1.0.0");
  const [stackUploading, setStackUploading] = useState(false);
  const [stackProgress, setStackProgress] = useState(0);
  const [stackMsg, setStackMsg] = useState<string | null>(null);
  const [stackInfo, setStackInfo] = useState<string | null>(null);

  const [modalConnectOpen, setModalConnectOpen] = useState(false);
  const [modalAddServerOpen, setModalAddServerOpen] = useState(false);
  const [bundleInput, setBundleInput] = useState("");
  const [addUrlInput, setAddUrlInput] = useState("");
  const [modalErr, setModalErr] = useState<string | null>(null);

  const [removeId, setRemoveId] = useState<string | null>(null);
  const [copied, setCopied] = useState(false);

  const [connectionSwitching, setConnectionSwitching] = useState(false);

  const [renameOpen, setRenameOpen] = useState(false);
  const [renameId, setRenameId] = useState<string | null>(null);
  const [renameLabel, setRenameLabel] = useState("");
  const [renameErr, setRenameErr] = useState<string | null>(null);

  /** HTTP control-api base for `/api/v1/host-stats/series` (not gRPC :50051). */
  const [controlApiInput, setControlApiInput] = useState("");

  const loadBookmarks = useCallback(async () => {
    try {
      // await invoke<ServerBookmark[]>("list_server_bookmarks");
      const list = await invoke<ServerBookmark[]>("list_server_bookmarks");
      setBookmarks(list);
    } catch {
      setBookmarks([]);
    }
  }, []);

  const refreshServer = useCallback(async () => {
    setServerLoading(true);
    setGrpcErr(null);
    try {
      // await invoke<GrpcConnectResult>("refresh_grpc_status");
      const r = await invoke<GrpcConnectResult>("refresh_grpc_status");
      setGrpcLive(r);
      setEndpoint(r.endpoint);
    } catch (e) {
      setGrpcErr(String(e));
      setGrpcLive(null);
    } finally {
      setServerLoading(false);
    }
  }, []);

  const init = useCallback(async () => {
    try {
      const ep = await invoke<string | null>("get_saved_grpc_endpoint");
      setEndpoint(ep);
      try {
        const cap = await invoke<string | null>("get_control_api_base");
        setControlApiInput(cap ?? "");
      } catch {
        setControlApiInput("");
      }
      if (ep) {
        try {
          const r = await invoke<GrpcConnectResult>("refresh_grpc_status");
          setGrpcLive(r);
          setGrpcErr(null);
        } catch (e) {
          setGrpcLive(null);
          setGrpcErr(String(e));
        }
      }
    } catch {
      setEndpoint(null);
    }
    await loadBookmarks();
  }, [loadBookmarks]);

  useEffect(() => {
    void init();
  }, [init]);

  useEffect(() => {
    let cancelled = false;
    let unlisten: (() => void) | undefined;
    void listen<{ sent: number; total: number }>("server_stack_upload_progress", (ev) => {
      const { sent, total } = ev.payload;
      const pct = total > 0 ? (sent / total) * 100 : 0;
      setStackProgress(Math.min(100, pct));
    }).then((fn) => {
      if (cancelled) fn();
      else unlisten = fn;
    });
    return () => {
      cancelled = true;
      unlisten?.();
    };
  }, []);

  const copyEndpoint = useCallback(() => {
    if (!endpoint) return;
    void navigator.clipboard.writeText(endpoint);
    setCopied(true);
    window.setTimeout(() => setCopied(false), 2000);
  }, [endpoint]);

  const loadMetrics = useCallback(async () => {
    setMetricsErr(null);
    setMetricsLoading(true);
    setUseMockMetrics(false);
    try {
      // Real integration (backend returns JSON string):
      // const raw = await invoke<string>("fetch_remote_host_stats");
      const raw = await invoke<string>("fetch_remote_host_stats");
      setMetrics(parseHostStatsSnapshot(raw));
    } catch (e) {
      setMetricsErr(String(e));
      setMetrics(MOCK_HOST_STATS);
      setUseMockMetrics(true);
    } finally {
      setMetricsLoading(false);
    }
  }, []);

  const onConnectSubmit = async () => {
    setModalErr(null);
    const bundle = bundleInput.trim();
    if (!bundle) {
      setModalErr("Paste a bundle or minimal JSON with url.");
      return;
    }
    try {
      // await invoke<GrpcConnectResult>("connect_grpc_bundle", { bundle });
      const r = await invoke<GrpcConnectResult>("connect_grpc_bundle", { bundle });
      setEndpoint(r.endpoint);
      setGrpcLive(r);
      setGrpcErr(null);
      setModalConnectOpen(false);
      setBundleInput("");
      await loadBookmarks();
    } catch (e) {
      setModalErr(String(e));
    }
  };

  const onAddServer = async () => {
    setModalErr(null);
    const url = addUrlInput.trim();
    if (!url) {
      setModalErr("Enter a gRPC URL (http://…).");
      return;
    }
    try {
      // await invoke<ServerBookmark>("add_server_bookmark", { url });
      await invoke<ServerBookmark>("add_server_bookmark", { url });
      setModalAddServerOpen(false);
      setAddUrlInput("");
      await loadBookmarks();
    } catch (e) {
      setModalErr(String(e));
    }
  };

  const onUseBookmark = async (url: string) => {
    if (endpoint && normalizeGrpcUrl(url) === normalizeGrpcUrl(endpoint)) {
      return;
    }
    setGrpcErr(null);
    setConnectionSwitching(true);
    try {
      const r = await invoke<GrpcConnectResult>("activate_server_bookmark", { url });
      setEndpoint(r.endpoint);
      setGrpcLive(r);
      await loadBookmarks();
    } catch (e) {
      setGrpcErr(String(e));
    } finally {
      setConnectionSwitching(false);
    }
  };

  const onSavedConnectionSelect = (value: string) => {
    if (!value) return;
    void onUseBookmark(value);
  };

  const onRenameSubmit = async () => {
    if (!renameId) return;
    setRenameErr(null);
    const label = renameLabel.trim();
    if (!label) {
      setRenameErr("Enter a label.");
      return;
    }
    try {
      await invoke("rename_server_bookmark", { id: renameId, label });
      setRenameOpen(false);
      setRenameId(null);
      setRenameLabel("");
      await loadBookmarks();
    } catch (e) {
      setRenameErr(String(e));
    }
  };

  const onRemoveBookmark = async () => {
    if (!removeId) return;
    try {
      // await invoke("delete_server_bookmark", { id: removeId });
      await invoke("delete_server_bookmark", { id: removeId });
      setRemoveId(null);
      await loadBookmarks();
    } catch (e) {
      setGrpcErr(String(e));
    }
  };

  const onDisconnect = async () => {
    try {
      // await invoke("clear_grpc_connection");
      await invoke("clear_grpc_connection");
      setEndpoint(null);
      setGrpcLive(null);
      setGrpcErr(null);
      setMetrics(null);
      setControlApiInput("");
    } catch (e) {
      setGrpcErr(String(e));
    }
  };

  const saveControlApiBase = useCallback(async () => {
    setGrpcErr(null);
    try {
      await invoke("set_control_api_base", { url: controlApiInput.trim() });
    } catch (e) {
      setGrpcErr(String(e));
    }
  }, [controlApiInput]);

  const onPickFolder = async () => {
    try {
      // await invoke<string | null>("pick_deploy_directory");
      const p = await invoke<string | null>("pick_deploy_directory");
      setDeployDir(p ?? null);
      if (!p) setDeployMsg("No folder selected.");
    } catch (e) {
      setDeployMsg(String(e));
    }
  };

  const onDeploy = async () => {
    if (!deployDir || !deployVersion.trim()) {
      setDeployMsg("Choose a folder and enter a version.");
      return;
    }
    setDeploying(true);
    setDeployProgress(0);
    setDeployCancelRequested(false);
    setDeployMsg("Packaging and uploading…");
    const steps = window.setInterval(() => {
      setDeployProgress((p) => Math.min(92, p + 6));
    }, 220);
    try {
      await invoke("set_active_project", { project_id: deployProject });
      // await invoke<DeployOutcome>("deploy_from_directory", { directory, version, chunkSize: null });
      const r = await invoke<DeployOutcome>("deploy_from_directory", {
        directory: deployDir,
        version: deployVersion.trim(),
        chunkSize: null,
      });
      if (deployCancelRequested) {
        setDeployMsg("Deploy finished on server; cancel only stopped local UI wait.");
      } else {
        setDeployMsg(
          `OK: ${r.status} → ${r.deployedVersion} (${r.artifactBytes} bytes, ${r.chunkCount} chunks)`,
        );
      }
      setDeployProgress(100);
      try {
        const live = await invoke<GrpcConnectResult>("refresh_grpc_status");
        setGrpcLive(live);
      } catch {
        /* ignore */
      }
    } catch (e) {
      setDeployMsg(String(e));
    } finally {
      window.clearInterval(steps);
      setDeploying(false);
      setTimeout(() => setDeployProgress(0), 800);
    }
  };

  const onPickStackTar = async () => {
    try {
      const p = await invoke<string | null>("pick_server_stack_tar_gz");
      setStackPath(p ?? null);
      if (!p) setStackMsg("No bundle file selected.");
      else setStackMsg(null);
    } catch (e) {
      setStackMsg(String(e));
    }
  };

  const onPickStackFolder = async () => {
    try {
      const p = await invoke<string | null>("pick_deploy_directory");
      setStackPath(p ?? null);
      if (!p) setStackMsg("No folder selected.");
      else setStackMsg(null);
    } catch (e) {
      setStackMsg(String(e));
    }
  };

  const onFetchStackInfo = async () => {
    try {
      const raw = await invoke<string>("fetch_server_stack_info_cmd");
      setStackInfo(raw);
    } catch (e) {
      setStackInfo(null);
      setStackMsg(String(e));
    }
  };

  const onApplyServerStack = async () => {
    if (!stackPath || !stackVersion.trim()) {
      setStackMsg("Choose a .tar.gz or extracted pirate-linux-amd64 folder and enter a version.");
      return;
    }
    setStackUploading(true);
    setStackProgress(0);
    setStackMsg("Uploading…");
    try {
      const r = await invoke<ServerStackOutcome>("apply_server_stack_update", {
        path: stackPath,
        version: stackVersion.trim(),
        chunkSize: null,
      });
      let msg = `OK: ${r.status} → ${r.appliedVersion}`;
      if (r.deployServerPkgVersion) {
        msg += ` (deploy-server ${r.deployServerPkgVersion}`;
        if (r.controlApiPkgVersion) msg += `, control-api ${r.controlApiPkgVersion}`;
        msg += ")";
      }
      setStackMsg(msg);
      setStackProgress(100);
      try {
        const live = await invoke<GrpcConnectResult>("refresh_grpc_status");
        setGrpcLive(live);
      } catch {
        setStackMsg(
          `${msg} — gRPC may drop briefly while deploy-server restarts; refresh status in a few seconds.`,
        );
      }
      await onFetchStackInfo();
    } catch (e) {
      setStackMsg(String(e));
    } finally {
      setStackUploading(false);
      setTimeout(() => setStackProgress(0), 1200);
    }
  };

  const liveState = grpcLive?.state?.toLowerCase() ?? "";
  const isRunning = liveState === "running";

  const selectedBookmarkUrl = useMemo(() => {
    if (!endpoint) return "";
    const n = normalizeGrpcUrl(endpoint);
    const hit = bookmarks.find((b) => normalizeGrpcUrl(b.url) === n);
    return hit ? hit.url : "";
  }, [endpoint, bookmarks]);

  return (
    <div className="min-h-screen bg-gradient-to-b from-deep via-[#100408] to-deep pb-16 text-slate-100">
      <div className="mx-auto max-w-[1600px] px-4 pt-8 sm:px-6 sm:pt-10 lg:px-10">
        <header className="mb-8 flex flex-col gap-4 sm:flex-row sm:items-end sm:justify-between">
          <nav
            className="inline-flex rounded-2xl border border-white/10 bg-black/25 p-1 shadow-inner"
            aria-label="Разделы приложения"
          >
            <button
              type="button"
              onClick={() => setMainTab("overview")}
              className={`inline-flex items-center gap-2 rounded-xl px-4 py-2 text-sm font-semibold transition ${
                mainTab === "overview"
                  ? "bg-gradient-to-r from-red-800/90 to-red-950/90 text-white shadow-md"
                  : "text-slate-400 hover:text-slate-100"
              }`}
            >
              <LayoutDashboard className="h-4 w-4" aria-hidden />
              Обзор
            </button>
            <button
              type="button"
              onClick={() => setMainTab("connection")}
              className={`inline-flex items-center gap-2 rounded-xl px-4 py-2 text-sm font-semibold transition ${
                mainTab === "connection"
                  ? "bg-gradient-to-r from-red-800/90 to-red-950/90 text-white shadow-md"
                  : "text-slate-400 hover:text-slate-100"
              }`}
            >
              <Link2 className="h-4 w-4" aria-hidden />
              Соединение
            </button>
            <button
              type="button"
              onClick={() => setMainTab("internet")}
              className={`inline-flex items-center gap-2 rounded-xl px-4 py-2 text-sm font-semibold transition ${
                mainTab === "internet"
                  ? "bg-gradient-to-r from-red-800/90 to-red-950/90 text-white shadow-md"
                  : "text-slate-400 hover:text-slate-100"
              }`}
            >
              <Globe className="h-4 w-4" aria-hidden />
              Интернет
            </button>
          </nav>
        </header>

        <div className="grid grid-cols-1 gap-6 xl:grid-cols-2 xl:gap-8 xl:items-start">
          {/* Слева: на «Обзоре» — краткий статус gRPC + метрики; на «Соединении» — полная панель deploy-server; «Интернет» — локальный прокси */}
          <div className="flex flex-col gap-6">
            {mainTab === "internet" ? (
              <InternetTrafficPanel />
            ) : (
              <>
            {mainTab === "overview" ? (
              <section
                className="rounded-2xl border border-white/10 bg-surface/90 p-4 shadow-card backdrop-blur"
                aria-labelledby="conn-summary-heading"
              >
                <h2
                  id="conn-summary-heading"
                  className="text-xs font-medium uppercase tracking-wide text-slate-500"
                >
                  Deploy-server (gRPC)
                </h2>
                <div className="mt-2 flex flex-col gap-3 sm:flex-row sm:items-center sm:justify-between">
                  <div className="min-w-0 flex-1">
                    {endpoint ? (
                      <code className="block truncate rounded-lg bg-black/40 px-2 py-1.5 text-sm text-amber-100/90">
                        {endpoint}
                      </code>
                    ) : (
                      <p className="text-sm text-slate-400">Не подключено к deploy-server.</p>
                    )}
                    <p className="mt-1 text-xs text-slate-500">
                      Статус:{" "}
                      <span className={isRunning ? "text-emerald-400" : "text-rose-300"}>
                        {grpcLive?.state ?? "—"}
                      </span>
                      {grpcLive?.currentVersion ? (
                        <span className="text-slate-500">
                          {" "}
                          · версия{" "}
                          <span className="text-slate-300">{grpcLive.currentVersion}</span>
                        </span>
                      ) : null}
                    </p>
                  </div>
                  <button
                    type="button"
                    onClick={() => setMainTab("connection")}
                    className={`${btnBase} shrink-0 border border-red-600/50 bg-red-950/40 text-red-100 hover:bg-red-950/60`}
                  >
                    Настройка соединения
                  </button>
                </div>
              </section>
            ) : null}
            {/* 1. Полная панель gRPC — только вкладка «Соединение» */}
            {mainTab === "connection" ? (
            <section
              className="rounded-2xl border border-white/10 bg-surface/90 p-5 shadow-card backdrop-blur transition hover:shadow-glow"
              aria-labelledby="server-heading"
            >
              <h2 id="server-heading" className="sr-only">
                Server connection
              </h2>
              {bookmarks.length > 0 ? (
                <div className="mb-4">
                  <label
                    htmlFor="saved-connection-select"
                    className="mb-1.5 block text-xs font-medium uppercase tracking-wide text-slate-500"
                  >
                    Saved connection
                  </label>
                  <select
                    id="saved-connection-select"
                    className="w-full rounded-xl border border-white/10 bg-black/30 px-3 py-2 text-sm text-slate-100 focus:border-red-600 focus:outline-none focus:ring-2 focus:ring-red-600/35 disabled:opacity-60"
                    value={selectedBookmarkUrl}
                    onChange={(e) => onSavedConnectionSelect(e.target.value)}
                    disabled={connectionSwitching}
                    aria-label="Saved gRPC connection"
                  >
                    <option value="">
                      {endpoint && !selectedBookmarkUrl
                        ? "Other connection (see endpoint below)"
                        : "— Choose —"}
                    </option>
                    {bookmarks.map((b) => (
                      <option key={b.id} value={b.url}>
                        {b.label}
                      </option>
                    ))}
                  </select>
                  {connectionSwitching ? (
                    <p className="mt-1.5 text-xs text-slate-500">Switching…</p>
                  ) : null}
                </div>
              ) : null}
              {!endpoint ? (
                <p className="text-sm text-slate-400">Not connected. Use Change to add an endpoint.</p>
              ) : (
                <div className="space-y-4">
                  <div className="flex flex-wrap items-center gap-2">
                    <Server className="h-5 w-5 text-red-400" aria-hidden />
                    <span className="text-xs font-medium uppercase tracking-wide text-slate-500">
                      Endpoint
                    </span>
                    <code className="max-w-full truncate rounded-lg bg-black/40 px-2 py-1 text-sm text-amber-100/90">
                      {endpoint}
                    </code>
                    <button
                      type="button"
                      onClick={copyEndpoint}
                      className={`${btnBase} shrink-0 border border-white/10 bg-white/5 p-2`}
                      title="Copy endpoint"
                    >
                      {copied ? (
                        <Check className="h-4 w-4 text-emerald-400" />
                      ) : (
                        <Copy className="h-4 w-4" />
                      )}
                    </button>
                  </div>
                  <div className="flex flex-wrap items-center gap-3">
                    <span
                      className={`inline-flex items-center gap-2 rounded-full px-3 py-1 text-sm font-medium ${
                        isRunning
                          ? "bg-emerald-500/15 text-emerald-300 ring-1 ring-emerald-500/40"
                          : "bg-rose-500/15 text-rose-200 ring-1 ring-rose-500/35"
                      }`}
                    >
                      <span
                        className={`h-2.5 w-2.5 rounded-full ${
                          isRunning ? "bg-emerald-400 shadow-[0_0_8px_#34d399]" : "bg-rose-500"
                        }`}
                        aria-hidden
                      />
                      Live: {grpcLive?.state ?? "—"}
                    </span>
                    <span className="text-sm text-slate-400">
                      version{" "}
                      <strong className="text-slate-200">
                        {grpcLive?.currentVersion || "—"}
                      </strong>
                    </span>
                  </div>
                  <div className="mt-4 rounded-xl border border-white/10 bg-black/20 p-3">
                    <p className="text-xs font-medium uppercase tracking-wide text-slate-500">
                      Control API (HTTP) — network charts
                    </p>
                    <p className="mt-1 text-xs text-slate-500">
                      gRPC deploy-server (e.g. :50051) does not serve REST. Enter the URL where{" "}
                      <code className="text-amber-200/70">control-api</code> listens (often :8080 or
                      nginx :443).
                    </p>
                    <div className="mt-2 flex flex-wrap gap-2">
                      <input
                        type="url"
                        value={controlApiInput}
                        onChange={(e) => setControlApiInput(e.target.value)}
                        placeholder="http://192.168.0.30:8080"
                        className="min-w-[12rem] flex-1 rounded-lg border border-white/10 bg-black/30 px-3 py-2 font-mono text-sm text-slate-100 placeholder:text-slate-600 focus:border-amber-600/50 focus:outline-none"
                      />
                      <button
                        type="button"
                        onClick={() => void saveControlApiBase()}
                        className={`${btnBase} border border-white/15 bg-white/5 text-slate-200 hover:bg-white/10`}
                      >
                        Save
                      </button>
                    </div>
                  </div>
                </div>
              )}
              <div className="mt-5 flex flex-wrap gap-2">
                <button
                  type="button"
                  disabled={serverLoading || !endpoint}
                  onClick={() => void refreshServer()}
                  className={`${btnBase} bg-gradient-to-r from-red-700 to-red-900 text-white shadow-lg shadow-red-950/40 hover:brightness-110 disabled:opacity-40`}
                >
                  {serverLoading ? (
                    <Loader2 className="h-4 w-4 animate-spin" />
                  ) : (
                    <RefreshCw className="h-4 w-4" />
                  )}
                  Refresh status
                </button>
                <button
                  type="button"
                  onClick={() => {
                    setModalConnectOpen(true);
                    setModalErr(null);
                  }}
                  className={`${btnBase} border border-white/15 bg-white/5 hover:bg-white/10`}
                >
                  Change…
                </button>
                <button
                  type="button"
                  disabled={!endpoint}
                  onClick={() => void onDisconnect()}
                  className={`${btnBase} border border-rose-500/40 bg-rose-500/10 text-rose-200 hover:bg-rose-500/20`}
                >
                  Disconnect
                </button>
              </div>
              {grpcErr ? (
                <p className="mt-3 flex items-start gap-2 text-sm text-rose-300">
                  <AlertCircle className="mt-0.5 h-4 w-4 shrink-0" />
                  {grpcErr}
                </p>
              ) : null}
            </section>
            ) : null}

            {/* 2. Remote host metrics — только «Обзор» */}
            {mainTab === "overview" ? (
              <>
            <HostMetricsPanel
              metrics={metrics}
              metricsLoading={metricsLoading}
              metricsErr={metricsErr}
              useMockMetrics={useMockMetrics}
              onLoad={() => void loadMetrics()}
              endpoint={endpoint}
              seriesBaseUrl={controlApiInput.trim() || null}
            />

            <DisplayStreamPanel />
              </>
            ) : null}
              </>
            )}
          </div>

          {/* Справа: на «Соединении» — закладки; на «Обзоре» — деплой и stack */}
          {mainTab !== "internet" ? (
          <div className="flex flex-col gap-6">
            {/* 3. Saved servers — только «Соединение» */}
            {mainTab === "connection" ? (
            <section
              className="rounded-2xl border border-white/10 bg-surface/90 p-5 shadow-card"
              aria-labelledby="bookmarks-heading"
            >
              <h2 id="bookmarks-heading" className="text-lg font-semibold text-slate-100">
                Saved servers
              </h2>
              <ul className="mt-4 space-y-3">
                {bookmarks.length === 0 ? (
                  <li className="rounded-xl border border-dashed border-white/10 p-4 text-center text-sm text-slate-500">
                    No saved servers yet.
                  </li>
                ) : (
                  bookmarks.map((b) => {
                    const isActive =
                      endpoint !== null && normalizeGrpcUrl(b.url) === normalizeGrpcUrl(endpoint);
                    return (
                      <li
                        key={b.id}
                        aria-current={isActive ? "true" : undefined}
                        className={`flex flex-col gap-3 rounded-xl border p-3 transition sm:flex-row sm:items-center sm:justify-between ${
                          isActive
                            ? "border-red-600/40 bg-red-950/30 ring-1 ring-red-500/50"
                            : "border-white/10 bg-black/20 hover:border-red-600/35"
                        }`}
                      >
                        <div className="min-w-0">
                          <div className="truncate text-sm font-medium text-slate-200">
                            {b.label}
                          </div>
                          <code className="break-all text-xs text-amber-200/80">{b.url}</code>
                        </div>
                        <div className="flex shrink-0 flex-wrap gap-2">
                          <button
                            type="button"
                            onClick={() => void onUseBookmark(b.url)}
                            disabled={isActive || connectionSwitching}
                            className={`${btnBase} border border-red-600/45 bg-red-950/40 px-3 py-2 text-red-100 hover:bg-red-950/60 disabled:pointer-events-none disabled:opacity-40`}
                          >
                            Use
                          </button>
                          <button
                            type="button"
                            onClick={() => {
                              setRenameId(b.id);
                              setRenameLabel(b.label);
                              setRenameErr(null);
                              setRenameOpen(true);
                            }}
                            className={`${btnBase} border border-white/10 bg-white/5 px-3 py-2 text-slate-300 hover:bg-white/10`}
                            title="Rename"
                          >
                            <Pencil className="h-4 w-4" />
                            <span className="sr-only">Rename</span>
                          </button>
                          <button
                            type="button"
                            onClick={() => setRemoveId(b.id)}
                            className={`${btnBase} border border-white/10 bg-white/5 px-3 py-2 text-rose-300 hover:bg-rose-500/10`}
                          >
                            <Trash2 className="h-4 w-4" />
                            <span className="sr-only">Remove</span>
                          </button>
                        </div>
                      </li>
                    );
                  })
                )}
              </ul>
              <button
                type="button"
                onClick={() => {
                  setModalAddServerOpen(true);
                  setModalErr(null);
                }}
                className={`${btnBase} mt-4 w-full border border-dashed border-white/20 bg-transparent text-slate-300 hover:border-red-600/50 hover:bg-red-950/30`}
              >
                <Plus className="h-4 w-4" />
                Add server
              </button>
            </section>
            ) : null}

            {mainTab === "overview" ? (
            <>
            {/* 4. Deploy artifact */}
            <section
              className="rounded-2xl border border-white/10 bg-surface/90 p-5 shadow-card"
              aria-labelledby="deploy-heading"
            >
              <h2 id="deploy-heading" className="text-lg font-semibold text-slate-100">
                Deploy artifact
              </h2>
              <p className="mt-2 text-sm text-slate-400">
                Packs the folder as tar.gz and uploads over gRPC (same as CLI{" "}
                <code className="rounded bg-black/40 px-1 text-amber-200/90">
                  client deploy
                </code>
                ).
              </p>
              {deployDir ? (
                <p className="mt-3 break-all text-sm text-emerald-300/90">
                  <FolderOpen className="mr-1 inline h-4 w-4" />
                  {deployDir}
                </p>
              ) : (
                <p className="mt-3 text-sm text-slate-500">No folder selected.</p>
              )}
              <div className="mt-4 grid gap-3 sm:grid-cols-2">
                <label className="block text-xs font-medium text-slate-500">
                  Version label
                  <input
                    value={deployVersion}
                    onChange={(e) => setDeployVersion(e.target.value)}
                    className="mt-1 w-full rounded-xl border border-white/10 bg-black/30 px-3 py-2 text-sm text-slate-100 focus:border-red-600 focus:outline-none focus:ring-2 focus:ring-red-600/35"
                    placeholder="v1.2.0"
                  />
                </label>
                <label className="block text-xs font-medium text-slate-500">
                  Project id
                  <input
                    value={deployProject}
                    onChange={(e) => setDeployProject(e.target.value)}
                    className="mt-1 w-full rounded-xl border border-white/10 bg-black/30 px-3 py-2 text-sm text-slate-100 focus:border-red-600 focus:outline-none focus:ring-2 focus:ring-red-600/35"
                    placeholder="default"
                  />
                </label>
              </div>
              <div className="mt-4 flex flex-wrap gap-2">
                <button
                  type="button"
                  disabled={deploying}
                  onClick={() => void onPickFolder()}
                  className={`${btnBase} border border-white/15 bg-white/5 hover:bg-white/10`}
                >
                  <FolderOpen className="h-4 w-4" />
                  Choose folder…
                </button>
                <button
                  type="button"
                  disabled={deploying || !deployDir}
                  onClick={() => void onDeploy()}
                  className={`${btnBase} bg-gradient-to-r from-red-800 to-amber-900 text-white shadow-md shadow-black/40 hover:brightness-110`}
                >
                  {deploying ? <Loader2 className="h-4 w-4 animate-spin" /> : null}
                  Deploy
                </button>
                {deploying ? (
                  <button
                    type="button"
                    onClick={() => {
                      setDeployCancelRequested(true);
                      setDeployMsg("Cancel requested — if upload already started, it may still complete.");
                    }}
                    className={`${btnBase} border border-rose-500/40 text-rose-200 hover:bg-rose-500/10`}
                  >
                    <X className="h-4 w-4" />
                    Cancel
                  </button>
                ) : null}
              </div>
              {deploying ? (
                <div className="mt-4">
                  <div className="mb-1 flex justify-between text-xs text-slate-500">
                    <span>Progress</span>
                    <span>{Math.round(deployProgress)}%</span>
                  </div>
                  <ProgressBar ratio={deployProgress / 100} />
                </div>
              ) : null}
              {deployMsg ? (
                <p className="mt-3 text-sm text-slate-400">{deployMsg}</p>
              ) : null}
              {/* Tauri: invoke('deploy_from_directory', { directory, version, chunkSize }) */}
            </section>

            {/* 5. Server stack OTA */}
            <section
              className="rounded-2xl border border-white/10 bg-surface/90 p-5 shadow-card"
              aria-labelledby="stack-heading"
            >
              <h2 id="stack-heading" className="text-lg font-semibold text-slate-100">
                Server stack update
              </h2>
              <p className="mt-2 text-sm text-slate-400">
                Upload the Linux bundle (e.g.{" "}
                <code className="rounded bg-black/40 px-1 text-amber-200/90">pirate-linux-amd64*.tar.gz</code>{" "}
                from <code className="rounded bg-black/40 px-1">build-linux-bundle.sh</code>). Requires{" "}
                <code className="rounded bg-black/40 px-1">DEPLOY_ALLOW_SERVER_STACK_UPDATE=1</code> on the
                host.
              </p>
              <p className="mt-1 text-xs text-slate-500">
                Local UI release (repo <code className="rounded bg-black/30 px-1">VERSION</code>):{" "}
                {import.meta.env.VITE_APP_RELEASE}
              </p>
              {stackPath ? (
                <p className="mt-3 break-all text-sm text-emerald-300/90">
                  <FolderOpen className="mr-1 inline h-4 w-4" />
                  {stackPath}
                </p>
              ) : (
                <p className="mt-3 text-sm text-slate-500">No bundle path selected.</p>
              )}
              <label className="mt-4 block text-xs font-medium text-slate-500">
                Stack version label
                <input
                  value={stackVersion}
                  onChange={(e) => setStackVersion(e.target.value)}
                  className="mt-1 w-full rounded-xl border border-white/10 bg-black/30 px-3 py-2 text-sm text-slate-100 focus:border-red-600 focus:outline-none focus:ring-2 focus:ring-red-600/35"
                  placeholder="20260411"
                />
              </label>
              <div className="mt-4 flex flex-wrap gap-2">
                <button
                  type="button"
                  disabled={stackUploading}
                  onClick={() => void onPickStackTar()}
                  className={`${btnBase} border border-white/15 bg-white/5 hover:bg-white/10`}
                >
                  <FileArchive className="h-4 w-4" />
                  .tar.gz…
                </button>
                <button
                  type="button"
                  disabled={stackUploading}
                  onClick={() => void onPickStackFolder()}
                  className={`${btnBase} border border-white/15 bg-white/5 hover:bg-white/10`}
                >
                  <FolderOpen className="h-4 w-4" />
                  Folder…
                </button>
                <button
                  type="button"
                  disabled={stackUploading || !stackPath}
                  onClick={() => void onApplyServerStack()}
                  className={`${btnBase} bg-gradient-to-r from-amber-900 to-red-900 text-white shadow-md shadow-black/40 hover:brightness-110`}
                >
                  {stackUploading ? <Loader2 className="h-4 w-4 animate-spin" /> : null}
                  Apply stack
                </button>
                <button
                  type="button"
                  disabled={stackUploading}
                  onClick={() => void onFetchStackInfo()}
                  className={`${btnBase} border border-white/15 bg-white/5 hover:bg-white/10`}
                >
                  <RefreshCw className="h-4 w-4" />
                  Stack info
                </button>
              </div>
              {stackUploading ? (
                <div className="mt-4">
                  <div className="mb-1 flex justify-between text-xs text-slate-500">
                    <span>Upload</span>
                    <span>{Math.round(stackProgress)}%</span>
                  </div>
                  <ProgressBar ratio={stackProgress / 100} />
                </div>
              ) : null}
              {stackInfo ? (
                <pre className="mt-3 max-h-32 overflow-auto rounded-lg bg-black/40 p-2 text-xs text-slate-400">
                  {stackInfo}
                </pre>
              ) : null}
              {stackMsg ? (
                <p className="mt-3 text-sm text-slate-400">{stackMsg}</p>
              ) : null}
            </section>
            </>
            ) : null}
          </div>
        ) : null}
        </div>
      </div>

      {/* Modal: connect / change endpoint */}
      {modalConnectOpen ? (
        <div
          className="fixed inset-0 z-40 flex items-center justify-center bg-black/70 p-4 backdrop-blur-sm"
          role="dialog"
          aria-modal="true"
          onClick={(e) => e.target === e.currentTarget && setModalConnectOpen(false)}
        >
          <div className="w-full max-w-lg rounded-2xl border border-white/10 bg-surface p-6 shadow-2xl">
            <h3 className="text-lg font-semibold text-slate-100">Connect to deploy-server</h3>
            <p className="mt-2 text-sm text-slate-400">
              Paste install JSON or <code className="rounded bg-black/30 px-1">{"{ \"url\": \"http://…\" }"}</code>
            </p>
            <textarea
              value={bundleInput}
              onChange={(e) => setBundleInput(e.target.value)}
              rows={6}
              className="mt-3 w-full resize-y rounded-xl border border-white/10 bg-black/40 px-3 py-2 font-mono text-sm text-slate-100 focus:border-red-600 focus:outline-none focus:ring-2 focus:ring-red-600/35"
              placeholder='{"url":"http://127.0.0.1:50051"}'
            />
            {modalErr ? <p className="mt-2 text-sm text-rose-400">{modalErr}</p> : null}
            <div className="mt-4 flex justify-end gap-2">
              <button
                type="button"
                onClick={() => setModalConnectOpen(false)}
                className={`${btnBase} border border-white/10 bg-white/5`}
              >
                Cancel
              </button>
              <button
                type="button"
                onClick={() => void onConnectSubmit()}
                className={`${btnBase} bg-gradient-to-r from-red-700 to-red-950 text-white shadow-md shadow-red-950/50`}
              >
                Connect
              </button>
            </div>
          </div>
        </div>
      ) : null}

      {/* Modal: add server */}
      {modalAddServerOpen ? (
        <div
          className="fixed inset-0 z-40 flex items-center justify-center bg-black/70 p-4 backdrop-blur-sm"
          role="dialog"
          aria-modal="true"
          onClick={(e) => e.target === e.currentTarget && setModalAddServerOpen(false)}
        >
          <div className="w-full max-w-md rounded-2xl border border-white/10 bg-surface p-6 shadow-2xl">
            <h3 className="text-lg font-semibold text-slate-100">Add server</h3>
            <input
              value={addUrlInput}
              onChange={(e) => setAddUrlInput(e.target.value)}
              className="mt-3 w-full rounded-xl border border-white/10 bg-black/40 px-3 py-2 text-sm text-slate-100 focus:border-red-600 focus:outline-none focus:ring-2 focus:ring-red-600/35"
              placeholder="http://192.168.0.30:50051"
            />
            {modalErr ? <p className="mt-2 text-sm text-rose-400">{modalErr}</p> : null}
            <div className="mt-4 flex justify-end gap-2">
              <button
                type="button"
                onClick={() => setModalAddServerOpen(false)}
                className={`${btnBase} border border-white/10 bg-white/5`}
              >
                Cancel
              </button>
              <button
                type="button"
                onClick={() => void onAddServer()}
                className={`${btnBase} bg-gradient-to-r from-red-700 to-red-950 text-white shadow-md shadow-red-950/50`}
              >
                Save
              </button>
            </div>
          </div>
        </div>
      ) : null}

      {/* Modal: rename bookmark */}
      {renameOpen ? (
        <div
          className="fixed inset-0 z-40 flex items-center justify-center bg-black/70 p-4 backdrop-blur-sm"
          role="dialog"
          aria-modal="true"
          aria-labelledby="rename-bookmark-title"
          onClick={(e) => e.target === e.currentTarget && setRenameOpen(false)}
        >
          <div className="w-full max-w-md rounded-2xl border border-white/10 bg-surface p-6 shadow-2xl">
            <h3 id="rename-bookmark-title" className="text-lg font-semibold text-slate-100">
              Rename server
            </h3>
            <label className="mt-3 block text-xs font-medium text-slate-500" htmlFor="rename-bookmark-input">
              Label
            </label>
            <input
              id="rename-bookmark-input"
              value={renameLabel}
              onChange={(e) => setRenameLabel(e.target.value)}
              className="mt-1 w-full rounded-xl border border-white/10 bg-black/40 px-3 py-2 text-sm text-slate-100 focus:border-red-600 focus:outline-none focus:ring-2 focus:ring-red-600/35"
              placeholder="Production"
            />
            {renameErr ? <p className="mt-2 text-sm text-rose-400">{renameErr}</p> : null}
            <div className="mt-4 flex justify-end gap-2">
              <button
                type="button"
                onClick={() => {
                  setRenameOpen(false);
                  setRenameId(null);
                  setRenameErr(null);
                }}
                className={`${btnBase} border border-white/10 bg-white/5`}
              >
                Cancel
              </button>
              <button
                type="button"
                onClick={() => void onRenameSubmit()}
                className={`${btnBase} bg-gradient-to-r from-red-700 to-red-950 text-white shadow-md shadow-red-950/50`}
              >
                Save
              </button>
            </div>
          </div>
        </div>
      ) : null}

      {/* Confirm remove */}
      {removeId ? (
        <div className="fixed inset-0 z-50 flex items-center justify-center bg-black/75 p-4" role="alertdialog">
          <div className="w-full max-w-sm rounded-2xl border border-white/10 bg-surface p-6 shadow-2xl">
            <h3 className="font-semibold text-slate-100">Remove server?</h3>
            <p className="mt-2 text-sm text-slate-400">This cannot be undone.</p>
            <div className="mt-4 flex justify-end gap-2">
              <button
                type="button"
                onClick={() => setRemoveId(null)}
                className={`${btnBase} border border-white/10 bg-white/5`}
              >
                Cancel
              </button>
              <button
                type="button"
                onClick={() => void onRemoveBookmark()}
                className={`${btnBase} bg-rose-600 text-white hover:bg-rose-500`}
              >
                Remove
              </button>
            </div>
          </div>
        </div>
      ) : null}
    </div>
  );
}

