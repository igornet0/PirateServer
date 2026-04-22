/**
 * PirateClient desktop shell — главный экран.
 *
 * Интеграция с Tauri (вызываются из этого файла):
 * - `get_saved_grpc_endpoint` — сохранённый URL gRPC
 * - `get_control_api_base` / `set_control_api_base` — HTTP base control-api (графики series)
 * - `refresh_grpc_status` / `connect_grpc_bundle` / `clear_grpc_connection` — соединение
 * - `fetch_remote_host_stats` / `fetch_remote_host_stats_detail` — метрики хоста (gRPC)
 * - `list_server_bookmarks` / `add_server_bookmark` / `delete_server_bookmark` / `activate_server_bookmark` / `rename_server_bookmark`
 * - `pick_deploy_directory` / `set_active_project` / `deploy_from_directory` / `rollback_deploy`
 * - `projects_preflight` — проверки перед деплоем (JSON)
 * - `list_registered_projects` / `register_project_from_directory` / `remove_registered_project` — локальный реестр папок
 * - `deploy_upload_cancel` / `server_stack_upload_cancel` — прерывание потока чанков (best-effort)
 * - `deploy-progress` — этапы деплоя (prepare/archive/upload/apply) и байты при отправке
 * - `pick_server_stack_tar_gz` / `apply_server_stack_update` / `fetch_server_stack_info_cmd` (OTA host bundle)
 * - `pirate_cli_path_info` / `install_pirate_cli` — сравнение версии CLI в PATH с встроенной; напоминание обновить после апдейта приложения
 * - `start_display_ingest` / `display_ingest_base` / `display_ingest_export_consumer_config` — display stream receive
 * - `get_display_stream_prefs` / `set_display_stream_prefs` — local stream send/receive flags
 * - `internet_proxy_start` / `internet_proxy_stop` / `internet_proxy_status` — локальный CONNECT-прокси
 * - `load_client_settings_json` / `save_client_settings_json` / `apply_default_rules_preset_cmd` — settings.json и пресеты правил
 *
 * Вкладки: «Обзор», «Проекты», «Соединение», «Интернет» (прокси и правила как у `pirate board`).
 *
 * Примеры будущих расширений (закомментируйте и подключите при необходимости):
 * // await invoke("get_metrics");
 * // await invoke("deploy_artifact", { ... });
 * // await invoke("change_endpoint", { url });
 */
import { invoke } from "@tauri-apps/api/core";
import { listen } from "@tauri-apps/api/event";
import {
  AlertCircle,
  Check,
  Copy,
  FileArchive,
  FolderOpen,
  Loader2,
  Plus,
  RefreshCw,
  Server,
  Settings,
  Terminal,
  Trash2,
  X,
} from "lucide-react";
import React, { useCallback, useEffect, useMemo, useRef, useState } from "react";
import { toast } from "sonner";
import { Toaster } from "sonner";
import { AppShell } from "./AppShell";
import { DisplayStreamPanel } from "./DisplayStreamPanel";
import { InternetTrafficPanel } from "./InternetTrafficPanel";
import { HostMetricsPanel } from "./HostMetricsPanel";
import { OverviewContextPanel } from "./OverviewContextPanel";
import { ProjectSwitcher } from "./ProjectSwitcher";
import { ProjectsPanel } from "./ProjectsPanel";
import { ServerProjectsOverview } from "./ServerProjectsOverview";
import { ServerBookmarkSettingsModal } from "./ServerBookmarkSettingsModal";
import { SidebarNav, type MainTab } from "./SidebarNav";
import pirateAppIcon from "../src-tauri/icons/icon.png";
import { useI18n } from "./i18n";
import type { HostServicesCompatSummary } from "./projects-preflight-types";
import type { ToolchainReport } from "./toolchain-types";
import {
  MOCK_HOST_STATS,
  parseHostStatsSnapshot,
  type HostStatsSnapshot,
} from "./host-stats-types";
import { suggestControlApiFromGrpcUrl } from "./controlApiUrl";
import { ModalDialog } from "./ui/ModalDialog";

// -----------------------------------------------------------------------------
// Types & mock data (used when Tauri is unavailable or for Storybook-style preview)
// -----------------------------------------------------------------------------

/** Tauri `pirate_cli_path_info` (camelCase). */
type PirateCliPathInfo = {
  pathInPath: string | null;
  pathVersion: string | null;
  bundledVersion: string | null;
  needsUpdate: boolean;
};

type GrpcConnectResult = {
  endpoint: string;
  currentVersion: string;
  projectVersion: string;
  state: string;
  /** HTTP base from server (nginx/public); matches Rust `control_api_http_url`. */
  controlApiHttpUrl?: string;
  /** Direct control-api base; matches Rust `control_api_http_url_direct`. */
  controlApiHttpUrlDirect?: string;
};

type ServerBookmark = {
  id: string;
  label: string;
  url: string;
  host_agent_base_url?: string | null;
  host_agent_token?: string | null;
};

type DeployOutcome = {
  status: string;
  deployedVersion: string;
  artifactBytes: number;
  chunkCount: number;
  /** `grpc` | `http_chunked` | `http_multipart` when the desktop chose a transport */
  uploadChannel?: string;
};

function formatArtifactSize(bytes: number): string {
  if (!Number.isFinite(bytes) || bytes < 0) return String(bytes);
  const mib = bytes / (1024 * 1024);
  if (mib >= 0.01 && mib < 1024) return `${mib.toFixed(2)} MiB (${bytes} B)`;
  const gib = bytes / (1024 * 1024 * 1024);
  if (gib >= 1) return `${gib.toFixed(2)} GiB (${bytes} B)`;
  return `${bytes} B`;
}

/** Payload from Tauri `deploy-progress` (Rust `DeployProgressEvent`, camelCase). */
type DeployProgressPayload = {
  phase: string;
  uploadSent?: number;
  uploadTotal?: number;
  /** Sub-status from desktop (resumable session, chunk retries, finalize). */
  detail?: string;
};

function deployUiPercent(ev: DeployProgressPayload): number {
  const p = ev.phase;
  if (p === "prepare") return 8;
  if (p === "archive") return 24;
  if (p === "upload") {
    const s = ev.uploadSent ?? 0;
    const t = ev.uploadTotal ?? 0;
    if (t > 0) return 30 + Math.min(62, Math.round((62 * s) / t));
    return 34;
  }
  if (p === "apply") return 96;
  return 4;
}

/** Default stack version label from bundle path: filename without `.tar.gz` (or `.tgz`). */
function stackVersionLabelFromBundlePath(filePath: string): string {
  const trimmed = filePath.trim();
  const base = trimmed.split(/[/\\]/).pop() ?? trimmed;
  return base.replace(/\.tar\.gz$/i, "").replace(/\.tgz$/i, "");
}

type ServerStackOutcome = {
  status: string;
  appliedVersion: string;
  deployServerPkgVersion?: string | null;
  controlApiPkgVersion?: string | null;
};

type ParsedStackInfo = {
  bundleVersion?: string;
  deployServerBinaryVersion?: string;
  hostDashboardEnabled?: boolean;
  hostGuiDetectedAtInstall?: string | null;
  hostGuiInstallJson?: string | null;
  hostNginxPirateSite?: boolean;
  manifestJson?: string;
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

function formatMaybe(value: unknown): string {
  if (value === null || value === undefined || value === "") return "—";
  if (typeof value === "boolean") return value ? "Yes" : "No";
  return String(value);
}

/** Typical control-api HTTP base from deploy-server gRPC URL (best-effort). */
/** Prefer public URL from GetStatus; fall back to direct (same logic as desktop `connection`). */
function controlApiHintFromGrpcLive(live: GrpcConnectResult): string | null {
  const pub = live.controlApiHttpUrl?.trim();
  if (pub) return pub;
  const direct = live.controlApiHttpUrlDirect?.trim();
  if (direct) return direct;
  return null;
}

/** `GetStatus.current_version` when no app release: `stack@…` is host bundle label, not a deploy version. */
function isServerStackIdleLabel(version: string): boolean {
  return version.trim().startsWith("stack@");
}

function formatDeployedVersionForUi(version: string): { label: string; value: string } {
  const v = version.trim();
  if (!v) return { label: "", value: "" };
  return isServerStackIdleLabel(v)
    ? { label: "сервер (bundle)", value: v }
    : { label: "активный релиз", value: v };
}

/** Tone for deploy-server managed **app process** (`running` | `stopped` | `error`), not gRPC connectivity. */
function grpcProcessToneFromState(stateRaw: string | undefined): {
  text: string;
  dot: string;
  badge: string;
} {
  const s = (stateRaw ?? "").toLowerCase();
  if (s === "running") {
    return {
      text: "text-orange-400",
      dot: "bg-orange-500 shadow-[0_0_10px_rgba(249,115,22,0.55)]",
      badge: "bg-orange-500/15 text-orange-200 ring-1 ring-orange-500/45",
    };
  }
  if (s === "error") {
    return {
      text: "text-rose-300",
      dot: "bg-rose-500",
      badge: "bg-rose-500/15 text-rose-200 ring-1 ring-rose-500/35",
    };
  }
  if (s === "stopped") {
    return {
      text: "text-red-300/90",
      dot: "bg-red-500",
      badge: "bg-red-950/40 text-red-200 ring-1 ring-red-600/35",
    };
  }
  return {
    text: "text-slate-300",
    dot: "bg-slate-400",
    badge: "bg-slate-500/15 text-slate-200 ring-1 ring-slate-500/35",
  };
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
        className="h-full rounded-full bg-gradient-to-r from-red-800 via-orange-600 to-red-700 shadow-flame transition-[width] duration-500 ease-out"
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

export function Dashboard() {
  const { language, setLanguage, t } = useI18n();
  const tr = (ru: string, en: string) => (language === "ru" ? ru : en);
    const [mainTab, setMainTab] = useState<MainTab>("projects");

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
  const [deploying, setDeploying] = useState(false);
  const [deployProgress, setDeployProgress] = useState(0);
  /** Current deploy pipeline stage from backend (`prepare` → `apply`). */
  const [deployPhaseKey, setDeployPhaseKey] = useState<string | null>(null);
  const [deployPhaseDetail, setDeployPhaseDetail] = useState<string | null>(null);
  const [deployMsg, setDeployMsg] = useState<string | null>(null);
  const [deployCancelRequested, setDeployCancelRequested] = useState(false);
  const deployingRef = useRef(false);
  useEffect(() => {
    deployingRef.current = deploying;
  }, [deploying]);

  useEffect(() => {
    let unlisten: (() => void) | undefined;
    let cancelled = false;
    void listen<DeployProgressPayload>("deploy-progress", (event) => {
      if (!deployingRef.current) return;
      const ev = event.payload;
      setDeployPhaseKey(ev.phase);
      const d = ev.detail?.trim();
      setDeployPhaseDetail(d ? d : null);
      setDeployProgress(deployUiPercent(ev));
    }).then((fn) => {
      if (cancelled) fn();
      else unlisten = fn;
    });
    return () => {
      cancelled = true;
      unlisten?.();
    };
  }, []);

  const [paasMsg, setPaasMsg] = useState<string | null>(null);
  const [paasBusy, setPaasBusy] = useState(false);

  const [registryRefreshKey, setRegistryRefreshKey] = useState(0);
  const bumpRegistryRefresh = useCallback(() => {
    setRegistryRefreshKey((k) => k + 1);
  }, []);

  const [toolchainReport, setToolchainReport] = useState<ToolchainReport | null>(null);
  const [toolchainLoading, setToolchainLoading] = useState(true);
  const [toolchainErr, setToolchainErr] = useState<string | null>(null);

  const refreshToolchain = useCallback(async () => {
    setToolchainLoading(true);
    setToolchainErr(null);
    try {
      const r = await invoke<ToolchainReport>("probe_local_toolchain");
      setToolchainReport(r);
    } catch (e) {
      setToolchainErr(String(e));
      setToolchainReport(null);
    } finally {
      setToolchainLoading(false);
    }
  }, []);

  const [stackPath, setStackPath] = useState<string | null>(null);
  const [stackVersion, setStackVersion] = useState("stack-1.0.0");
  const [stackUploading, setStackUploading] = useState(false);
  const [stackProgress, setStackProgress] = useState(0);
  const [stackMsg, setStackMsg] = useState<string | null>(null);
  const [stackInfo, setStackInfo] = useState<string | null>(null);
  const [stackModalOpen, setStackModalOpen] = useState(false);
  const [stackTargetUrl, setStackTargetUrl] = useState<string | null>(null);
  const [stackTargetLabel, setStackTargetLabel] = useState<string | null>(null);

  const [modalConnectOpen, setModalConnectOpen] = useState(false);
  /** When true, connect modal shows guided title (same form as «Change…»). */
  const [connectionWizardMode, setConnectionWizardMode] = useState(false);
  const [modalAddServerOpen, setModalAddServerOpen] = useState(false);
  const [bundleInput, setBundleInput] = useState("");
  const [addUrlInput, setAddUrlInput] = useState("");
  const [modalErr, setModalErr] = useState<string | null>(null);

  const [removeId, setRemoveId] = useState<string | null>(null);
  const hostSvcResolverRef = useRef<((v: boolean) => void) | null>(null);
  const [hostSvcPromptMessage, setHostSvcPromptMessage] = useState<string | null>(null);
  const [copied, setCopied] = useState(false);

  const [connectionSwitching, setConnectionSwitching] = useState(false);

  const [serverSettingsBookmark, setServerSettingsBookmark] = useState<ServerBookmark | null>(null);

  const [pirateCliPromptOpen, setPirateCliPromptOpen] = useState(false);
  const [pirateCliUpdatePromptOpen, setPirateCliUpdatePromptOpen] = useState(false);
  const [pirateCliUpdateInfo, setPirateCliUpdateInfo] = useState<PirateCliPathInfo | null>(null);
  const [pirateCliInstallBusy, setPirateCliInstallBusy] = useState(false);
  const [pirateCliInstallResult, setPirateCliInstallResult] = useState<string | null>(null);
  const [pirateCliInstallResultErr, setPirateCliInstallResultErr] = useState(false);

  /** HTTP control-api base for `/api/v1/host-stats/series` (not gRPC :50051). */
  const [controlApiInput, setControlApiInput] = useState("");

  const commitGrpcLive = useCallback((live: GrpcConnectResult) => {
    setGrpcLive(live);
    const hint = controlApiHintFromGrpcLive(live);
    if (hint) {
      setControlApiInput((prev) => {
        const p = normalizeGrpcUrl(prev);
        const h = normalizeGrpcUrl(hint);
        if (p === h) return prev;
        return hint;
      });
      return;
    }
    const inferred = suggestControlApiFromGrpcUrl(live.endpoint);
    if (!inferred) return;
    setControlApiInput((prev) => {
      if (prev.trim() !== "") return prev;
      return inferred;
    });
  }, []);

  const loadBookmarks = useCallback(async () => {
    try {
      // await invoke<ServerBookmark[]>("list_server_bookmarks");
      const list = await invoke<ServerBookmark[]>("list_server_bookmarks");
      setBookmarks(list);
    } catch {
      setBookmarks([]);
    }
  }, []);

  useEffect(() => {
    setServerSettingsBookmark((prev) => {
      if (!prev) return null;
      const next = bookmarks.find((b) => b.id === prev.id);
      return next ?? null;
    });
  }, [bookmarks]);

  const refreshServer = useCallback(async () => {
    setServerLoading(true);
    setGrpcErr(null);
    try {
      // await invoke<GrpcConnectResult>("refresh_grpc_status");
      const r = await invoke<GrpcConnectResult>("refresh_grpc_status");
      commitGrpcLive(r);
      setEndpoint(r.endpoint);
    } catch (e) {
      setGrpcErr(String(e));
      setGrpcLive(null);
    } finally {
      setServerLoading(false);
    }
  }, [commitGrpcLive]);

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
          commitGrpcLive(r);
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
  }, [loadBookmarks, commitGrpcLive]);

  useEffect(() => {
    void init();
  }, [init]);

  useEffect(() => {
    let cancelled = false;
    (async () => {
      setToolchainLoading(true);
      setToolchainErr(null);
      try {
        const r = await invoke<ToolchainReport>("probe_local_toolchain");
        if (!cancelled) setToolchainReport(r);
      } catch (e) {
        if (!cancelled) {
          setToolchainErr(String(e));
          setToolchainReport(null);
        }
      } finally {
        if (!cancelled) setToolchainLoading(false);
      }
    })();
    return () => {
      cancelled = true;
    };
  }, []);

  useEffect(() => {
    let cancelled = false;
    (async () => {
      try {
        const dismissedMissing = localStorage.getItem("pirateDesktop.pirateCliPromptDismissed") === "1";
        const available = await invoke<boolean>("is_pirate_cli_available");
        if (cancelled) return;
        if (!available) {
          if (!dismissedMissing) {
            setPirateCliInstallResult(null);
            setPirateCliInstallResultErr(false);
            setPirateCliPromptOpen(true);
          }
          return;
        }
        const info = await invoke<PirateCliPathInfo>("pirate_cli_path_info");
        if (cancelled) return;
        const dismissedUp = localStorage.getItem("pirateDesktop.pirateCliUpdateDismissedFor") ?? "";
        if (
          info.needsUpdate &&
          info.bundledVersion &&
          dismissedUp !== info.bundledVersion
        ) {
          setPirateCliUpdateInfo(info);
          setPirateCliInstallResult(null);
          setPirateCliInstallResultErr(false);
          setPirateCliUpdatePromptOpen(true);
        }
      } catch {
        /* Tauri unavailable (browser dev) or command missing */
      }
    })();
    return () => {
      cancelled = true;
    };
  }, []);

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

  const resolveHostSvcPrompt = useCallback((v: boolean) => {
    const fn = hostSvcResolverRef.current;
    hostSvcResolverRef.current = null;
    setHostSvcPromptMessage(null);
    fn?.(v);
  }, []);

  const waitHostSvcConfirm = useCallback((message: string) => {
    return new Promise<boolean>((resolve) => {
      hostSvcResolverRef.current = resolve;
      setHostSvcPromptMessage(message);
    });
  }, []);

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
      commitGrpcLive(r);
      setGrpcErr(null);
      setModalConnectOpen(false);
      setConnectionWizardMode(false);
      setBundleInput("");
      await loadBookmarks();
      toast.message(
        tr(
          "Следующий шаг: HTTP Control API",
          "Next step: HTTP Control API",
        ),
        {
          description: tr(
            "Вкладка «Соединение» → шестерёнка у сервера → войдите в control-api для графиков и REST.",
            "Connection tab → gear next to the server → sign in to control-api for charts and REST.",
          ),
          action: {
            label: tr("Открыть «Соединение»", "Open Connection"),
            onClick: () => setMainTab("connection"),
          },
        },
      );
    } catch (e) {
      setModalErr(String(e));
    }
  };

  const onAddServer = async () => {
    setModalErr(null);
    const pasted = addUrlInput.trim();
    if (!pasted) {
      setModalErr(t("auto.Dashboard_tsx.1"));
      return;
    }
    try {
      await invoke<ServerBookmark>("add_server_bookmark", { url: pasted });
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
      commitGrpcLive(r);
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

  useEffect(() => {
    let cancelled = false;
    if (!deployDir?.trim()) return;
    void (async () => {
      try {
        const v = await invoke<string>("read_release_version_from_manifest", {
          directory: deployDir,
        });
        if (!cancelled && v.trim()) {
          setDeployVersion(v.trim());
        }
      } catch {
        // keep manually entered/default version when manifest is missing/invalid
      }
    })();
    return () => {
      cancelled = true;
    };
  }, [deployDir]);

  const onDeploy = async () => {
    if (!deployDir || !deployVersion.trim()) {
      setDeployMsg("Choose a folder and enter a version.");
      return;
    }
    try {
      await invoke<string>("ensure_deploy_project_id_for_deploy", { path: deployDir });
    } catch (e) {
      setDeployMsg(String(e));
      return;
    }
    try {
      const analysis = await invoke<{ hostServices: HostServicesCompatSummary }>("analyze_network_access", {
        directory: deployDir,
      });
      const hs = analysis.hostServices;
      if (hs.status === "checked" && hs.missingHostServiceIds.length > 0) {
        const list = hs.missingHostServiceIds.join(", ");
        const ok = await waitHostSvcConfirm(
          tr(
            `На сервере не установлены пакеты: ${list}. Установите их через закладку сервера → «Сервисы». Продолжить деплой?`,
            `Missing host packages: ${list}. Install via Server bookmark → Services tab. Deploy anyway?`,
          ),
        );
        if (!ok) {
          setDeployMsg(
            tr("Деплой отменён (хост-сервисы).", "Deploy cancelled (host services)."),
          );
          return;
        }
      } else if (hs.status === "skipped" && hs.requiredHostServiceIds.length > 0) {
        const list = hs.requiredHostServiceIds.join(", ");
        const ok = await waitHostSvcConfirm(
          tr(
            `Проект требует на хосте: ${list}. Не удалось проверить без входа в control-api. Продолжить деплой?`,
            `Project requires on host: ${list}. Could not verify without control-api login. Deploy anyway?`,
          ),
        );
        if (!ok) {
          setDeployMsg(
            tr("Деплой отменён (хост-сервисы).", "Deploy cancelled (host services)."),
          );
          return;
        }
      }
    } catch {
      /* proceed with deploy if analysis fails */
    }
    setDeploying(true);
    setDeployProgress(4);
    setDeployPhaseKey("prepare");
    setDeployPhaseDetail(null);
    setDeployCancelRequested(false);
    setDeployMsg(
      tr("Идёт деплой на сервер…", "Deploying to server…"),
    );
    try {
      // await invoke<DeployOutcome>("deploy_from_directory", { directory, version, chunkSize: null });
      const r = await invoke<DeployOutcome>("deploy_from_directory", {
        directory: deployDir,
        version: deployVersion.trim(),
        chunkSize: null,
      });
      if (deployCancelRequested) {
        setDeployMsg("Deploy finished on server; cancel only stopped local UI wait.");
        toast.message(t("auto.Dashboard_tsx.2"), {
          description: t("auto.Dashboard_tsx.3"),
        });
      } else {
        setDeployMsg(
          `OK: ${r.status} → ${r.deployedVersion} (${formatArtifactSize(r.artifactBytes)}, ${r.chunkCount} chunks${r.uploadChannel ? `, ${r.uploadChannel}` : ""})`,
        );
        toast.success(t("auto.Dashboard_tsx.4"), { description: r.deployedVersion });
      }
      setDeployProgress(100);
      try {
        const live = await invoke<GrpcConnectResult>("refresh_grpc_status");
        commitGrpcLive(live);
      } catch {
        /* ignore */
      }
    } catch (e) {
      const msg = String(e);
      setDeployMsg(msg);
      toast.error(t("auto.Dashboard_tsx.5"), { description: msg });
    } finally {
      setDeploying(false);
      setDeployPhaseKey(null);
      setDeployPhaseDetail(null);
      setTimeout(() => {
        setDeployProgress(0);
      }, 800);
    }
  };

  const paasPath = deployDir;
  const runPaas = async (
    label: string,
    fn: () => Promise<string | void>,
    opts?: { onSuccess?: () => void },
  ) => {
    if (!paasPath) {
      setPaasMsg("Choose a project folder first (same as Deploy).");
      return;
    }
    setPaasBusy(true);
    setPaasMsg(`${label}…`);
    try {
      const out = await fn();
      setPaasMsg(typeof out === "string" ? out : `${label} ${t("auto.Dashboard_tsx.6")}`);
      opts?.onSuccess?.();
    } catch (e) {
      setPaasMsg(String(e));
    } finally {
      setPaasBusy(false);
    }
  };

  const onDeployCancelRequest = () => {
    setDeployCancelRequested(true);
    setDeployMsg(t("auto.Dashboard_tsx.7"));
  };

  const onPipelineFull = async () => {
    if (!deployDir || !deployVersion.trim()) {
      setPaasMsg(t("auto.Dashboard_tsx.8"));
      return;
    }
    try {
      await invoke<string>("ensure_deploy_project_id_for_deploy", { path: deployDir });
    } catch (e) {
      setPaasMsg(String(e));
      return;
    }
    try {
      const analysis = await invoke<{ hostServices: HostServicesCompatSummary }>("analyze_network_access", {
        directory: deployDir,
      });
      const hs = analysis.hostServices;
      if (hs.status === "checked" && hs.missingHostServiceIds.length > 0) {
        const list = hs.missingHostServiceIds.join(", ");
        const ok = await waitHostSvcConfirm(
          tr(
            `На сервере не установлены пакеты: ${list}. Установите их через закладку сервера → «Сервисы». Продолжить pipeline?`,
            `Missing host packages: ${list}. Install via Server bookmark → Services tab. Continue pipeline?`,
          ),
        );
        if (!ok) {
          setPaasMsg(tr("Pipeline отменён (хост-сервисы).", "Pipeline cancelled (host services)."));
          return;
        }
      } else if (hs.status === "skipped" && hs.requiredHostServiceIds.length > 0) {
        const list = hs.requiredHostServiceIds.join(", ");
        const ok = await waitHostSvcConfirm(
          tr(
            `Проект требует на хосте: ${list}. Не удалось проверить без входа в control-api. Продолжить pipeline?`,
            `Project requires on host: ${list}. Could not verify without control-api login. Continue pipeline?`,
          ),
        );
        if (!ok) {
          setPaasMsg(tr("Pipeline отменён (хост-сервисы).", "Pipeline cancelled (host services)."));
          return;
        }
      }
    } catch {
      /* continue */
    }
    await runPaas("Pipeline", async () => {
      const s = await invoke<string>("paas_pipeline", {
        path: deployDir,
        doInit: false,
        name: null,
        skipTestLocal: true,
        version: deployVersion.trim(),
        chunkSize: null,
      });
      try {
        const live = await invoke<GrpcConnectResult>("refresh_grpc_status");
        commitGrpcLive(live);
      } catch {
        /* ignore */
      }
      return s;
    });
  };

  const onPickStackTar = async () => {
    try {
      const p = await invoke<string | null>("pick_server_stack_tar_gz");
      setStackPath(p ?? null);
      if (!p) setStackMsg("No bundle file selected.");
      else {
        setStackMsg(null);
        setStackVersion(stackVersionLabelFromBundlePath(p));
      }
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
      setStackMsg("Choose a .tar.gz bundle and enter a version.");
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
        commitGrpcLive(live);
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

  const onOpenStackUpdate = async (bookmark?: ServerBookmark) => {
    if (bookmark) {
      setStackTargetUrl(bookmark.url);
      setStackTargetLabel(bookmark.label);
      if (!endpoint || normalizeGrpcUrl(bookmark.url) !== normalizeGrpcUrl(endpoint)) {
        await onUseBookmark(bookmark.url);
      }
    } else {
      setStackTargetUrl(endpoint);
      setStackTargetLabel(null);
    }
    setStackModalOpen(true);
  };

  const grpcProcessTone = useMemo(
    () => grpcProcessToneFromState(grpcLive?.state),
    [grpcLive?.state],
  );

  const deployedVersionUi = useMemo(() => {
    const raw = grpcLive?.currentVersion?.trim();
    if (!raw) return null;
    return formatDeployedVersionForUi(raw);
  }, [grpcLive?.currentVersion]);

  const selectedBookmarkUrl = useMemo(() => {
    if (!endpoint) return "";
    const n = normalizeGrpcUrl(endpoint);
    const hit = bookmarks.find((b) => normalizeGrpcUrl(b.url) === n);
    return hit ? hit.url : "";
  }, [endpoint, bookmarks]);

  const removeBookmarkEntry = useMemo(
    () => (removeId ? bookmarks.find((b) => b.id === removeId) ?? null : null),
    [bookmarks, removeId],
  );

  const parsedStackInfo = useMemo(() => {
    if (!stackInfo) return null;
    try {
      return JSON.parse(stackInfo) as ParsedStackInfo;
    } catch {
      return null;
    }
  }, [stackInfo]);

  const parsedManifest = useMemo(() => {
    if (!parsedStackInfo?.manifestJson) return null;
    try {
      return JSON.parse(parsedStackInfo.manifestJson) as Record<string, unknown>;
    } catch {
      return null;
    }
  }, [parsedStackInfo]);

  const stackUiBundled = useMemo(() => {
    const raw = parsedManifest?.dashboard_ui_bundled;
    if (typeof raw === "boolean") return raw;
    if (typeof raw === "number") return raw !== 0;
    if (typeof raw === "string") {
      const t = raw.trim().toLowerCase();
      if (t === "0" || t === "false" || t === "no" || t === "off") return false;
      if (t === "1" || t === "true" || t === "yes" || t === "on") return true;
    }
    return null;
  }, [parsedManifest]);

  const serverSettingsHostUiBundled = useMemo(() => {
    if (!serverSettingsBookmark || !endpoint) return null;
    const same = normalizeGrpcUrl(serverSettingsBookmark.url) === normalizeGrpcUrl(endpoint);
    return same ? stackUiBundled : null;
  }, [serverSettingsBookmark, endpoint, stackUiBundled]);

  /** Anchor for modals that should dim only the workspace column (not the left sidebar). */
  const [workspacePortalEl, setWorkspacePortalEl] = useState<HTMLDivElement | null>(null);

  return (
    <>
      <Toaster
        position="top-right"
        theme="dark"
        richColors
        closeButton
        toastOptions={{
          classNames: {
            toast: "border border-red-900/50 bg-zinc-950/95 text-slate-100 shadow-glow backdrop-blur-sm",
            error: "border-rose-900/60",
            success: "border-emerald-900/50",
          },
        }}
      />
      <AppShell
        sidebar={
          <aside className="relative flex w-[272px] shrink-0 flex-col border-r border-red-950/40 bg-panel shadow-[inset_-1px_0_0_rgba(127,29,29,0.15)]">
            <div className="border-b border-border-subtle px-3 py-3">
              <div className="flex items-center gap-3">
                <img
                  src={pirateAppIcon}
                  alt=""
                  width={44}
                  height={44}
                  className="h-11 w-11 shrink-0 rounded-full ring-2 ring-red-900/70 shadow-glow"
                />
                <div className="min-w-0">
                  <h1 className="font-display text-xl leading-none tracking-wide text-red-400 drop-shadow-[0_0_12px_rgba(220,38,38,0.45)]">
                    PirateClient
                  </h1>
                  <p className="mt-1 text-[10px] leading-snug text-slate-500">
                    {t("auto.Dashboard_tsx.9")}
                  </p>
                </div>
              </div>
            </div>
            <SidebarNav mainTab={mainTab} onTab={setMainTab} />
            <ProjectSwitcher
              refreshKey={registryRefreshKey}
              currentDeployDir={deployDir}
              onSelectPath={(path) => setDeployDir(path)}
              onRegistryChanged={bumpRegistryRefresh}
            />
          </aside>
        }
        workspace={
          <div
            ref={setWorkspacePortalEl}
            className="relative flex min-h-0 flex-1 flex-col bg-app"
          >
            <header className="flex shrink-0 flex-wrap items-center justify-between gap-3 border-b border-border-subtle bg-panel/95 px-4 py-2.5 backdrop-blur-sm">
              <div className="min-w-0 flex flex-wrap items-center gap-3">
                {endpoint ? (
                  <code
                    title={endpoint}
                    className="max-w-[min(100%,28rem)] truncate rounded border border-red-900/35 bg-black/40 px-2 py-1 font-mono text-xs text-orange-200/90 shadow-[inset_0_0_0_1px_rgba(220,38,38,0.12)]"
                  >
                    {endpoint}
                  </code>
                ) : (
                  <span className="text-xs text-slate-500">
                    {t("dashboard.serverDisconnected")}
                  </span>
                )}
                {grpcLive ? (
                  <span className={`text-xs ${grpcProcessTone.text}`}>{t("auto.Dashboard_tsx.10")}: {grpcLive.state ?? "—"}</span>
                ) : null}
              </div>
              <div className="flex items-center gap-2">
                <div className="inline-flex rounded-lg border border-border-subtle bg-black/20 p-0.5">
                  <button
                    type="button"
                    onClick={() => setLanguage("ru")}
                    className={`rounded px-2 py-1 text-[11px] font-semibold ${
                      language === "ru" ? "bg-red-900/60 text-white" : "text-slate-400 hover:text-slate-200"
                    }`}
                  >
                    {t("lang.ru")}
                  </button>
                  <button
                    type="button"
                    onClick={() => setLanguage("en")}
                    className={`rounded px-2 py-1 text-[11px] font-semibold ${
                      language === "en" ? "bg-red-900/60 text-white" : "text-slate-400 hover:text-slate-200"
                    }`}
                  >
                    {t("lang.en")}
                  </button>
                </div>
                <button
                  type="button"
                  onClick={() => setMainTab("connection")}
                  className={`${btnBase} shrink-0 border border-red-900/40 bg-red-950/30 px-3 py-1.5 text-xs text-red-100 hover:bg-red-950/50 hover:shadow-glow`}
                >
                  {t("dashboard.connection")}
                </button>
              </div>
            </header>
            <div
              className={`flex min-h-0 flex-1 flex-col overflow-hidden lg:flex-row ${connectionSwitching ? "cursor-wait" : ""}`}
              aria-busy={connectionSwitching}
            >
              <div className="flex min-h-0 min-w-0 flex-1 flex-col overflow-hidden">
                {mainTab === "projects" ? (
                  <ProjectsPanel
                    deployDir={deployDir}
                    deployVersion={deployVersion}
                    deploying={deploying}
                    deployProgress={deployProgress}
                    deployPhaseKey={deployPhaseKey}
                    deployPhaseDetail={deployPhaseDetail}
                    deployMsg={deployMsg}
                    deployCancelRequested={deployCancelRequested}
                    paasBusy={paasBusy}
                    paasMsg={paasMsg}
                    endpoint={endpoint}
                    onSetDeployVersion={setDeployVersion}
                    onPickFolder={() => void onPickFolder()}
                    onDeploy={() => void onDeploy()}
                    onDeployCancelRequest={onDeployCancelRequest}
                    onPipelineFull={() => {
                      void onPipelineFull();
                    }}
                    onAfterRollback={() => {
                      void refreshServer();
                    }}
                    runPaas={runPaas}
                    onSelectProjectPath={(path) => setDeployDir(path)}
                    registryRefreshKey={registryRefreshKey}
                    onRegistryChanged={bumpRegistryRefresh}
                    toolchainReport={toolchainReport}
                    toolchainLoading={toolchainLoading}
                    toolchainErr={toolchainErr}
                    onRefreshToolchain={() => {
                      void refreshToolchain();
                    }}
                  />
                ) : (
                  <div className="min-h-0 flex-1 overflow-y-auto px-4 py-4 md:px-6">
                    <div
                      className={mainTab === "overview" ? "contents" : "hidden"}
                      aria-hidden={mainTab !== "overview"}
                    >
                      <section
                        className="rounded-lg border border-border-subtle bg-panel p-4 shadow-card"
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
                              <code className="block truncate rounded-lg border border-border-subtle bg-black/30 px-2 py-1.5 text-sm text-orange-200/90">
                                {endpoint}
                              </code>
                            ) : (
                              <p className="text-sm text-slate-400">{t("auto.Dashboard_tsx.11")}</p>
                            )}
                            <div className="mt-1 space-y-1 text-xs text-slate-500">
                              {endpoint ? (
                                grpcLive ? (
                                  <p>
                                    <span className="text-orange-400/95">{t("auto.Dashboard_tsx.12")}</span>
                                  </p>
                                ) : (
                                  <p className="text-slate-500">{t("auto.Dashboard_tsx.13")}</p>
                                )
                              ) : null}
                              {grpcLive ? (
                                <p>
                                  {t("auto.Dashboard_tsx.14")}:{" "}
                                  <span className={grpcProcessTone.text}>
                                    {grpcLive.state ?? "—"}
                                  </span>
                                  {deployedVersionUi ? (
                                    <span className="text-slate-500">
                                      {" "}
                                      · {deployedVersionUi.label}{" "}
                                      <span className="text-slate-300">{deployedVersionUi.value}</span>
                                    </span>
                                  ) : null}
                                  {grpcLive.projectVersion?.trim() ? (
                                    <span className="text-slate-500">
                                      {" "}
                                      · {t("auto.Dashboard_tsx.15")}{" "}
                                      <span className="text-slate-300">
                                        {grpcLive.projectVersion.trim()}
                                      </span>
                                    </span>
                                  ) : null}
                                </p>
                              ) : null}
                            </div>
                          </div>
                          <button
                            type="button"
                            onClick={() => setMainTab("connection")}
                            className={`${btnBase} shrink-0 border border-red-900/50 bg-red-950/40 text-orange-100 hover:bg-red-950/55`}
                          >
                            {t("auto.Dashboard_tsx.16")}
                          </button>
                        </div>
                      </section>
                      <ServerProjectsOverview
                        grpcEndpoint={endpoint}
                        controlApiBase={controlApiInput}
                        serverControlApiPublic={grpcLive?.controlApiHttpUrl?.trim() || null}
                        serverControlApiDirect={grpcLive?.controlApiHttpUrlDirect?.trim() || null}
                        modalPortalEl={workspacePortalEl}
                        onOpenConnectionSettings={() => setMainTab("connection")}
                        onOpenProjectDeploy={() => setMainTab("projects")}
                      />
                    </div>
                    {mainTab === "internet" ? <InternetTrafficPanel /> : null}
                    {mainTab === "overview" ? (
                      <div className="mt-6 flex flex-col gap-6">
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
                      </div>
                    ) : null}
                    {mainTab === "connection" ? (
                      <section
                        className="rounded-lg border border-border-subtle bg-panel p-5 shadow-card transition"
                        aria-labelledby="server-heading"
                      >
                        <h2 id="server-heading" className="sr-only">
                          {tr("Соединение с сервером", "Server connection")}
                        </h2>
                        {bookmarks.length > 0 ? (
                          <div className="mb-4">
                            <label
                              htmlFor="saved-connection-select"
                              className="mb-1.5 block text-xs font-medium uppercase tracking-wide text-slate-500"
                            >
                              {t("auto.Dashboard_tsx.17")}
                            </label>
                            <select
                              id="saved-connection-select"
                              className="w-full rounded-lg border border-border-subtle bg-black/30 px-3 py-2 text-sm text-slate-100 focus:border-red-600 focus:outline-none focus:ring-2 focus:ring-red-600/35 disabled:opacity-60"
                              value={selectedBookmarkUrl}
                              onChange={(e) => onSavedConnectionSelect(e.target.value)}
                              disabled={connectionSwitching}
                              aria-label={t("auto.Dashboard_tsx.18")}
                            >
                              <option value="">
                                {endpoint && !selectedBookmarkUrl
                                  ? t("auto.Dashboard_tsx.19")
                                  : t("auto.Dashboard_tsx.20")}
                              </option>
                              {bookmarks.map((b) => (
                                <option key={b.id} value={b.url}>
                                  {b.label}
                                </option>
                              ))}
                            </select>
                            {connectionSwitching ? (
                              <p className="mt-1.5 text-xs text-slate-500">{t("auto.Dashboard_tsx.21")}</p>
                            ) : null}
                          </div>
                        ) : null}
                        {!endpoint ? (
                          <p className="text-sm text-slate-400">{t("auto.Dashboard_tsx.22")}</p>
                        ) : (
                          <div className="space-y-4">
                            <div className="flex flex-wrap items-center gap-2">
                              <Server className="h-5 w-5 text-red-500/90" aria-hidden />
                              <span className="text-xs font-medium uppercase tracking-wide text-slate-500">
                                {t("auto.Dashboard_tsx.23")}
                              </span>
                              <code className="max-w-full truncate rounded-lg border border-border-subtle bg-black/40 px-2 py-1 text-sm text-orange-200/90">
                                {endpoint}
                              </code>
                              <button
                                type="button"
                                onClick={copyEndpoint}
                                className={`${btnBase} shrink-0 border border-border-subtle bg-panel-raised p-2`}
                                title={t("auto.Dashboard_tsx.24")}
                              >
                                {copied ? (
                                  <Check className="h-4 w-4 text-orange-400" />
                                ) : (
                                  <Copy className="h-4 w-4" />
                                )}
                              </button>
                            </div>
                            <div className="flex flex-wrap items-center gap-3">
                              <span
                                className={`inline-flex items-center gap-2 rounded-full px-3 py-1 text-sm font-medium ${grpcProcessTone.badge}`}
                              >
                                <span
                                  className={`h-2.5 w-2.5 rounded-full ${grpcProcessTone.dot}`}
                                  aria-hidden
                                />
                                {t("auto.Dashboard_tsx.25")}: {grpcLive?.state ?? "—"}
                              </span>
                              <span className="text-sm text-slate-400">
                                {deployedVersionUi ? (
                                  <>
                                    {deployedVersionUi.label}{" "}
                                    <strong className="text-slate-200">{deployedVersionUi.value}</strong>
                                  </>
                                ) : (
                                  <>
                                    {t("auto.Dashboard_tsx.26")}{" "}
                                    <strong className="text-slate-200">
                                      {grpcLive?.currentVersion || "—"}
                                    </strong>
                                  </>
                                )}
                              </span>
                              {grpcLive?.projectVersion?.trim() ? (
                                <span className="text-sm text-slate-400">
                                  · pirate.toml{" "}
                                  <strong className="text-slate-200">{grpcLive.projectVersion.trim()}</strong>
                                </span>
                              ) : null}
                            </div>
                          </div>
                        )}
                        <div className="mt-5 flex flex-wrap items-center justify-between gap-3">
                          <div className="flex flex-wrap gap-2">
                            <button
                              type="button"
                              disabled={serverLoading || !endpoint}
                              onClick={() => void refreshServer()}
                              className={`${btnBase} bg-red-800 text-white shadow-glow hover:bg-red-700 disabled:opacity-40`}
                            >
                              {serverLoading ? (
                                <Loader2 className="h-4 w-4 animate-spin" />
                              ) : (
                                <RefreshCw className="h-4 w-4" />
                              )}
                              {t("auto.Dashboard_tsx.27")}
                            </button>
                            <button
                              type="button"
                              onClick={() => {
                                setConnectionWizardMode(false);
                                setModalConnectOpen(true);
                                setModalErr(null);
                              }}
                              className={`${btnBase} border border-border-subtle bg-panel-raised text-slate-200 hover:bg-white/[0.06]`}
                            >
                              {t("auto.Dashboard_tsx.28")}
                            </button>
                            <button
                              type="button"
                              onClick={() => {
                                setConnectionWizardMode(true);
                                setModalConnectOpen(true);
                                setModalErr(null);
                              }}
                              className={`${btnBase} border border-red-900/45 bg-red-950/35 text-orange-100`}
                            >
                              {t("auto.Dashboard_tsx.29")}
                            </button>
                          </div>
                          <button
                            type="button"
                            disabled={!endpoint}
                            onClick={() => void onDisconnect()}
                            title={tr("Сбросить gRPC-сессию на этом компьютере", "Clear gRPC session on this machine")}
                            className={`${btnBase} shrink-0 border border-rose-500/40 bg-rose-500/10 text-rose-200 hover:bg-rose-500/20`}
                          >
                            {t("auto.Dashboard_tsx.30")}
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
                    {mainTab === "connection" ? (
                      <section
                        className="mt-6 rounded-lg border border-border-subtle bg-panel p-5 shadow-card"
                        aria-labelledby="bookmarks-heading"
                      >
                        <h2 id="bookmarks-heading" className="text-lg font-semibold text-slate-100">
                          {t("auto.Dashboard_tsx.31")}
                        </h2>
                        <ul className="mt-4 divide-y divide-border-subtle rounded-lg border border-border-subtle">
                          {bookmarks.length === 0 ? (
                            <li className="p-4 text-center">
                              <p className="text-sm text-slate-500">{t("auto.Dashboard_tsx.32")}</p>
                              <p className="mt-2 text-xs text-slate-600">
                                {tr(
                                  "Добавьте URL из install JSON или вставьте gRPC-адрес сервера.",
                                  "Add a URL from install JSON or paste the server gRPC address.",
                                )}
                              </p>
                              <button
                                type="button"
                                onClick={() => {
                                  setModalAddServerOpen(true);
                                  setModalErr(null);
                                }}
                                className={`${btnBase} mt-3 border border-red-900/50 bg-red-950/40 px-4 py-2 text-xs text-orange-100 hover:bg-red-950/55`}
                              >
                                <Plus className="h-4 w-4" />
                                {t("auto.Dashboard_tsx.38")}
                              </button>
                            </li>
                          ) : (
                            bookmarks.map((b) => {
                              const isActive =
                                endpoint !== null && normalizeGrpcUrl(b.url) === normalizeGrpcUrl(endpoint);
                              return (
                                <li
                                  key={b.id}
                                  aria-current={isActive ? "true" : undefined}
                                  className={`flex flex-col gap-2 px-3 py-2.5 sm:flex-row sm:items-center sm:justify-between ${
                                    isActive ? "bg-red-950/25" : ""
                                  }`}
                                >
                                  <div className="min-w-0">
                                    <div className="truncate text-sm font-medium text-slate-200">{b.label}</div>
                                    <code className="break-all text-xs text-orange-200/80">{b.url}</code>
                                  </div>
                                  <div className="flex shrink-0 flex-wrap gap-2">
                                    <button
                                      type="button"
                                      onClick={() => void onUseBookmark(b.url)}
                                      disabled={isActive || connectionSwitching}
                                      className={`${btnBase} border border-red-900/50 bg-red-950/40 px-3 py-1.5 text-xs text-orange-100 disabled:pointer-events-none disabled:opacity-40`}
                                    >
                                      {t("auto.Dashboard_tsx.33")}
                                    </button>
                                    <button
                                      type="button"
                                      disabled={connectionSwitching}
                                      onClick={() => void onOpenStackUpdate(b)}
                                      className={`${btnBase} border border-red-900/45 bg-red-950/30 px-3 py-1.5 text-xs text-orange-100 disabled:pointer-events-none disabled:opacity-40`}
                                    >
                                      <FileArchive className="h-4 w-4" />
                                      {t("auto.Dashboard_tsx.34")}
                                    </button>
                                    <button
                                      type="button"
                                      disabled={connectionSwitching}
                                      onClick={() => setServerSettingsBookmark(b)}
                                      className={`${btnBase} border border-border-subtle bg-panel-raised px-3 py-1.5 text-xs text-slate-200 disabled:pointer-events-none disabled:opacity-40`}
                                      title={t("auto.Dashboard_tsx.35")}
                                    >
                                      <Settings className="h-4 w-4" />
                                      <span className="sr-only">{t("auto.Dashboard_tsx.36")}</span>
                                    </button>
                                    <button
                                      type="button"
                                      onClick={() => setRemoveId(b.id)}
                                      className={`${btnBase} border border-border-subtle bg-panel-raised px-3 py-1.5 text-xs text-rose-300 hover:bg-rose-500/10`}
                                    >
                                      <Trash2 className="h-4 w-4" />
                                      <span className="sr-only">{t("auto.Dashboard_tsx.37")}</span>
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
                          className={`${btnBase} mt-4 w-full border border-dashed border-border-subtle bg-transparent text-slate-300 hover:border-orange-500/45 hover:bg-red-950/25`}
                        >
                          <Plus className="h-4 w-4" />
                          {t("auto.Dashboard_tsx.38")}
                        </button>
                      </section>
                    ) : null}
                  </div>
                )}
              </div>
              {mainTab !== "projects" ? (
                <OverviewContextPanel
                  endpoint={endpoint}
                  grpcErr={grpcErr}
                  processBadgeClass={grpcProcessTone.badge}
                  processDotClass={grpcProcessTone.dot}
                  processStateLabel={grpcLive?.state ?? "—"}
                  deployedLabel={deployedVersionUi?.label ?? null}
                  deployedValue={
                    deployedVersionUi?.value ?? grpcLive?.currentVersion?.trim() ?? ""
                  }
                  projectVersion={grpcLive?.projectVersion?.trim() ?? null}
                  tab={mainTab === "overview" ? "overview" : mainTab === "connection" ? "connection" : "internet"}
                  onOpenConnection={() => setMainTab("connection")}
                />
              ) : null}
            </div>
          </div>
        }
      />

      {/* In-app confirm instead of window.confirm for host services (deploy / pipeline) */}
      {hostSvcPromptMessage ? (
        <ModalDialog
          open
          zClassName="z-modalConfirm"
          onClose={() => resolveHostSvcPrompt(false)}
          panelClassName="w-full max-w-md"
          role="alertdialog"
          aria-labelledby="host-svc-confirm-title"
        >
          <div className="rounded-2xl border border-white/10 bg-surface p-6 shadow-2xl">
            <h3 id="host-svc-confirm-title" className="text-lg font-semibold text-slate-100">
              {tr("Проверка хост-сервисов", "Host services")}
            </h3>
            <p className="mt-2 break-words text-sm text-slate-400">{hostSvcPromptMessage}</p>
            <div className="mt-4 flex justify-end gap-2">
              <button
                type="button"
                data-modal-initial-focus
                onClick={() => resolveHostSvcPrompt(false)}
                className={`${btnBase} border border-white/10 bg-white/5`}
              >
                {tr("Отмена", "Cancel")}
              </button>
              <button
                type="button"
                onClick={() => resolveHostSvcPrompt(true)}
                className={`${btnBase} bg-gradient-to-r from-red-700 to-red-950 text-white shadow-md shadow-red-950/50`}
              >
                {tr("Продолжить", "Continue")}
              </button>
            </div>
          </div>
        </ModalDialog>
      ) : null}

      {/* Modal: pirate CLI missing from PATH */}
      {pirateCliPromptOpen ? (
        <ModalDialog
          open
          zClassName="z-modalElevated"
          onClose={() => setPirateCliPromptOpen(false)}
          aria-labelledby="pirate-cli-prompt-title"
        >
          <div className="rounded-2xl border border-white/10 bg-surface p-6 shadow-2xl">
            <div className="flex items-start gap-3">
              <div className="mt-0.5 rounded-lg bg-orange-500/15 p-2 text-orange-200">
                <Terminal className="h-5 w-5" />
              </div>
              <div className="min-w-0 flex-1">
                <h3 id="pirate-cli-prompt-title" className="text-lg font-semibold text-slate-100">
                  Команда pirate не найдена в терминале
                </h3>
                <p className="mt-2 text-sm text-slate-400">
                  {t("auto.Dashboard_tsx.39")}{" "}
                  <code className="rounded bg-black/30 px-1">pirate auth</code> {t("auto.Dashboard_tsx.40")}{" "}
                  <code className="rounded bg-black/30 px-1">pirate</code>{" "}
                  {t("auto.Dashboard_tsx.41")}
                </p>
                {pirateCliInstallResult ? (
                  <p
                    className={`mt-3 rounded-xl border px-3 py-2 text-sm ${
                      pirateCliInstallResultErr
                        ? "border-rose-500/30 bg-rose-950/30 text-rose-100/90"
                        : "border-orange-600/35 bg-orange-950/25 text-orange-100/90"
                    }`}
                  >
                    {pirateCliInstallResult}
                  </p>
                ) : null}
              </div>
            </div>
            <div className="mt-5 flex flex-wrap justify-end gap-2">
              <button
                type="button"
                disabled={pirateCliInstallBusy}
                onClick={() => {
                  setPirateCliInstallResult(null);
                  setPirateCliInstallResultErr(false);
                  setPirateCliInstallBusy(true);
                  void (async () => {
                    try {
                      const msg = await invoke<string>("install_pirate_cli");
                      setPirateCliInstallResultErr(false);
                      setPirateCliInstallResult(msg);
                      const ok = await invoke<boolean>("is_pirate_cli_available");
                      if (ok) {
                        setPirateCliPromptOpen(false);
                      }
                    } catch (e) {
                      setPirateCliInstallResultErr(true);
                      setPirateCliInstallResult(String(e));
                    } finally {
                      setPirateCliInstallBusy(false);
                    }
                  })();
                }}
                className={`${btnBase} bg-gradient-to-r from-red-700 to-red-950 text-white shadow-md shadow-red-950/50 disabled:opacity-60`}
              >
                {pirateCliInstallBusy ? (
                  <Loader2 className="h-4 w-4 animate-spin" />
                ) : (
                  <Terminal className="h-4 w-4" />
                )}
                {tr("Установить pirate в PATH", "Install pirate to PATH")}
              </button>
              <button
                type="button"
                disabled={pirateCliInstallBusy}
                onClick={() => setPirateCliPromptOpen(false)}
                className={`${btnBase} border border-white/10 bg-white/5`}
              >
                {tr("Позже", "Later")}
              </button>
              <button
                type="button"
                disabled={pirateCliInstallBusy}
                onClick={() => {
                  localStorage.setItem("pirateDesktop.pirateCliPromptDismissed", "1");
                  setPirateCliPromptOpen(false);
                }}
                className={`${btnBase} border border-white/10 bg-white/5`}
              >
                {tr("Не напоминать", "Do not remind")}
              </button>
            </div>
          </div>
        </ModalDialog>
      ) : null}

      {/* Modal: PATH pirate older than bundled CLI (after app update) */}
      {pirateCliUpdatePromptOpen && pirateCliUpdateInfo ? (
        <ModalDialog
          open
          zClassName="z-modalElevated"
          onClose={() => setPirateCliUpdatePromptOpen(false)}
          aria-labelledby="pirate-cli-update-title"
        >
          <div className="rounded-2xl border border-white/10 bg-surface p-6 shadow-2xl">
            <div className="flex items-start gap-3">
              <div className="mt-0.5 rounded-lg bg-amber-500/15 p-2 text-amber-200">
                <RefreshCw className="h-5 w-5" />
              </div>
              <div className="min-w-0 flex-1">
                <h3 id="pirate-cli-update-title" className="text-lg font-semibold text-slate-100">
                  {tr("Обновить команду pirate в терминале", "Update the pirate terminal command")}
                </h3>
                <p className="mt-2 text-sm text-slate-400">
                  {tr(
                    "В PATH всё ещё старая версия CLI. Обновите установку, чтобы она совпадала с этим приложением.",
                    "The CLI on your PATH is still an older build. Refresh the install so it matches this app.",
                  )}
                </p>
                <dl className="mt-3 space-y-1 rounded-xl border border-white/10 bg-black/20 px-3 py-2 font-mono text-xs text-slate-300">
                  <div className="flex flex-wrap gap-x-2">
                    <dt className="text-slate-500">{tr("В приложении", "Bundled")}</dt>
                    <dd>{pirateCliUpdateInfo.bundledVersion ?? "—"}</dd>
                  </div>
                  <div className="flex flex-wrap gap-x-2">
                    <dt className="text-slate-500">{tr("В терминале (PATH)", "On PATH")}</dt>
                    <dd>{pirateCliUpdateInfo.pathVersion ?? "—"}</dd>
                  </div>
                  {pirateCliUpdateInfo.pathInPath ? (
                    <div className="break-all text-slate-500">
                      {pirateCliUpdateInfo.pathInPath}
                    </div>
                  ) : null}
                </dl>
                {pirateCliInstallResult ? (
                  <p
                    className={`mt-3 rounded-xl border px-3 py-2 text-sm ${
                      pirateCliInstallResultErr
                        ? "border-rose-500/30 bg-rose-950/30 text-rose-100/90"
                        : "border-emerald-600/35 bg-emerald-950/25 text-emerald-100/90"
                    }`}
                  >
                    {pirateCliInstallResult}
                  </p>
                ) : null}
              </div>
            </div>
            <div className="mt-5 flex flex-wrap justify-end gap-2">
              <button
                type="button"
                disabled={pirateCliInstallBusy}
                onClick={() => {
                  setPirateCliInstallResult(null);
                  setPirateCliInstallResultErr(false);
                  setPirateCliInstallBusy(true);
                  void (async () => {
                    try {
                      const msg = await invoke<string>("install_pirate_cli");
                      setPirateCliInstallResultErr(false);
                      setPirateCliInstallResult(msg);
                      const info = await invoke<PirateCliPathInfo>("pirate_cli_path_info");
                      if (!info.needsUpdate) {
                        setPirateCliUpdatePromptOpen(false);
                        setPirateCliUpdateInfo(null);
                      }
                    } catch (e) {
                      setPirateCliInstallResultErr(true);
                      setPirateCliInstallResult(String(e));
                    } finally {
                      setPirateCliInstallBusy(false);
                    }
                  })();
                }}
                className={`${btnBase} bg-gradient-to-r from-amber-700 to-amber-950 text-white shadow-md shadow-amber-950/50 disabled:opacity-60`}
              >
                {pirateCliInstallBusy ? (
                  <Loader2 className="h-4 w-4 animate-spin" />
                ) : (
                  <RefreshCw className="h-4 w-4" />
                )}
                {tr("Обновить pirate в PATH", "Update pirate in PATH")}
              </button>
              <button
                type="button"
                disabled={pirateCliInstallBusy}
                onClick={() => setPirateCliUpdatePromptOpen(false)}
                className={`${btnBase} border border-white/10 bg-white/5`}
              >
                {tr("Позже", "Later")}
              </button>
              <button
                type="button"
                disabled={pirateCliInstallBusy}
                onClick={() => {
                  const v = pirateCliUpdateInfo.bundledVersion;
                  if (v) {
                    localStorage.setItem("pirateDesktop.pirateCliUpdateDismissedFor", v);
                  }
                  setPirateCliUpdatePromptOpen(false);
                  setPirateCliUpdateInfo(null);
                }}
                className={`${btnBase} border border-white/10 bg-white/5`}
              >
                {tr("Не напоминать для этой версии", "Do not remind for this app version")}
              </button>
            </div>
          </div>
        </ModalDialog>
      ) : null}

      {/* Modal: connect / change endpoint */}
      {modalConnectOpen ? (
        <ModalDialog
          open
          zClassName="z-modal"
          onClose={() => {
            setModalConnectOpen(false);
            setConnectionWizardMode(false);
          }}
        >
          <div className="rounded-2xl border border-white/10 bg-surface p-6 shadow-2xl">
            <h3 className="text-lg font-semibold text-slate-100">
              {connectionWizardMode
                ? tr("Мастер подключения", "Connection wizard")
                : tr("Подключение к deploy-server", "Connect to deploy-server")}
            </h3>
            <p className="mt-2 text-sm text-slate-400">
              {connectionWizardMode
                ? tr(
                    "Вставьте install JSON с сервера или минимальный JSON с полем url. HTTP control-api для графиков и REST задайте в настройках сервера (шестеренка в списке сохраненных серверов на вкладке «Соединение»).",
                    "Paste install JSON from server or minimal JSON with url field. Set HTTP control-api for charts and REST in server settings (gear icon in saved servers list on Connection tab).",
                  )
                : tr("Вставьте install JSON или ", "Paste install JSON or ")}
              {!connectionWizardMode ? (
                <code className="rounded bg-black/30 px-1">{"{ \"url\": \"http://…\" }"}</code>
              ) : null}
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
                onClick={() => {
                  setModalConnectOpen(false);
                  setConnectionWizardMode(false);
                }}
                className={`${btnBase} border border-white/10 bg-white/5`}
              >
                {tr("Отмена", "Cancel")}
              </button>
              <button
                type="button"
                onClick={() => void onConnectSubmit()}
                className={`${btnBase} bg-gradient-to-r from-red-700 to-red-950 text-white shadow-md shadow-red-950/50`}
              >
                {connectionWizardMode ? tr("Подключить", "Connect") : tr("Подключить", "Connect")}
              </button>
            </div>
          </div>
        </ModalDialog>
      ) : null}

      {/* Modal: add server */}
      {modalAddServerOpen ? (
        <ModalDialog open zClassName="z-modal" onClose={() => setModalAddServerOpen(false)}>
          <div className="rounded-2xl border border-white/10 bg-surface p-6 shadow-2xl">
            <h3 className="text-lg font-semibold text-slate-100">{t("auto.Dashboard_tsx.42")}</h3>
            <p className="mt-2 text-sm text-slate-400">
              {t("auto.Dashboard_tsx.43")} <code className="rounded bg-black/30 px-1">http://host:50051</code>,{" "}
              {t("auto.Dashboard_tsx.44")}{" "}
              <code className="rounded bg-black/30 px-1">url</code> {t("auto.Dashboard_tsx.45")} (
              <code className="rounded bg-black/30 px-1">token</code>, <code className="rounded bg-black/30 px-1">
                url
              </code>
              , <code className="rounded bg-black/30 px-1">pairing</code>) — {t("auto.Dashboard_tsx.46")}
            </p>
            <textarea
              value={addUrlInput}
              onChange={(e) => setAddUrlInput(e.target.value)}
              rows={6}
              className="mt-3 w-full resize-y rounded-xl border border-white/10 bg-black/40 px-3 py-2 font-mono text-sm text-slate-100 placeholder:text-slate-600 focus:border-red-600 focus:outline-none focus:ring-2 focus:ring-red-600/35"
              placeholder={`http://192.168.0.30:50051\n\nили\n{"url":"http://192.168.0.30:50051"}`}
            />
            {modalErr ? <p className="mt-2 text-sm text-rose-400">{modalErr}</p> : null}
            <div className="mt-4 flex justify-end gap-2">
              <button
                type="button"
                onClick={() => setModalAddServerOpen(false)}
                className={`${btnBase} border border-white/10 bg-white/5`}
              >
                {t("auto.Dashboard_tsx.47")}
              </button>
              <button
                type="button"
                onClick={() => void onAddServer()}
                className={`${btnBase} bg-gradient-to-r from-red-700 to-red-950 text-white shadow-md shadow-red-950/50`}
              >
                {t("auto.Dashboard_tsx.48")}
              </button>
            </div>
          </div>
        </ModalDialog>
      ) : null}

      {serverSettingsBookmark ? (
        <ServerBookmarkSettingsModal
          open
          onClose={() => setServerSettingsBookmark(null)}
          bookmark={serverSettingsBookmark}
          activeEndpoint={endpoint}
          savedControlApiBase={controlApiInput}
          onBookmarkRenamed={loadBookmarks}
          hostUiBundled={serverSettingsHostUiBundled}
        />
      ) : null}

      {/* Modal: server stack update */}
      {stackModalOpen ? (
        <ModalDialog
          open
          zClassName="z-modal"
          onClose={() => {
            if (stackUploading) void invoke("server_stack_upload_cancel");
            setStackModalOpen(false);
          }}
          panelClassName="w-full max-w-2xl max-h-[90vh] min-h-0"
          aria-labelledby="stack-update-title"
        >
          <div className="max-h-[90vh] overflow-y-auto rounded-2xl border border-white/10 bg-surface p-6 shadow-2xl">
            <div className="flex items-start justify-between gap-4">
              <div>
                <h3 id="stack-update-title" className="text-lg font-semibold text-slate-100">
                  {t("auto.Dashboard_tsx.49")}
                </h3>
                <p className="mt-1 text-sm text-slate-400">
                  {stackTargetLabel ? (
                    <>
                      {t("auto.Dashboard_tsx.50")}: <span className="text-slate-200">{stackTargetLabel}</span>
                    </>
                  ) : (
                    t("auto.Dashboard_tsx.51")
                  )}
                </p>
                {stackTargetUrl ? (
                  <code className="mt-2 block break-all rounded bg-black/40 px-2 py-1 text-xs text-orange-200/90">
                    {stackTargetUrl}
                  </code>
                ) : null}
              </div>
              <button
                type="button"
                onClick={() => {
                  if (stackUploading) void invoke("server_stack_upload_cancel");
                  setStackModalOpen(false);
                }}
                className={`${btnBase} border border-white/10 bg-white/5 p-2`}
                title={t("auto.Dashboard_tsx.52")}
              >
                <X className="h-4 w-4" />
              </button>
            </div>

            <p className="mt-4 text-sm text-slate-400">
              {t("auto.Dashboard_tsx.53")}
              <code className="rounded bg-black/40 px-1 text-orange-200/90">pirate-linux-amd64*.tar.gz</code>{" "}
              {t("auto.Dashboard_tsx.54")}<code className="rounded bg-black/40 px-1">build-linux-bundle.sh</code>
              {t("auto.Dashboard_tsx.55")}
              <code className="rounded bg-black/40 px-1">DEPLOY_ALLOW_SERVER_STACK_UPDATE=1</code> {t("auto.Dashboard_tsx.56")}
            </p>
            <p className="mt-1 text-xs text-slate-500">
              {t("auto.Dashboard_tsx.57")}<code className="rounded bg-black/30 px-1">VERSION</code>):{" "}
              {import.meta.env.VITE_APP_RELEASE}
            </p>
            {stackPath ? (
              <p className="mt-3 break-all text-sm text-orange-300/90">
                <FolderOpen className="mr-1 inline h-4 w-4" />
                {stackPath}
              </p>
            ) : (
              <p className="mt-3 text-sm text-slate-500">{t("auto.Dashboard_tsx.58")}</p>
            )}
            <label className="mt-4 block text-xs font-medium text-slate-500">
              {t("auto.Dashboard_tsx.59")}
              <input
                value={stackVersion}
                onChange={(e) => setStackVersion(e.target.value)}
                className="mt-1 w-full rounded-xl border border-white/10 bg-black/30 px-3 py-2 text-sm text-slate-100 focus:border-red-600 focus:outline-none focus:ring-2 focus:ring-red-600/35"
                placeholder={tr("из имени .tar.gz при выборе файла", "from .tar.gz filename when you pick")}
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
                disabled={stackUploading || !stackPath}
                onClick={() => void onApplyServerStack()}
                className={`${btnBase} bg-gradient-to-r from-red-900 to-red-950 text-white shadow-md shadow-black/40 hover:brightness-110`}
              >
                {stackUploading ? <Loader2 className="h-4 w-4 animate-spin" /> : null}
                {t("auto.Dashboard_tsx.60")}
              </button>
              <button
                type="button"
                disabled={stackUploading}
                onClick={() => void onFetchStackInfo()}
                className={`${btnBase} border border-white/15 bg-white/5 hover:bg-white/10`}
              >
                <RefreshCw className="h-4 w-4" />
                {t("auto.Dashboard_tsx.61")}
              </button>
            </div>
            {stackUploading ? (
              <div className="mt-4">
                <div className="mb-1 flex justify-between text-xs text-slate-500">
                  <span>{t("auto.Dashboard_tsx.62")}</span>
                  <span>{Math.round(stackProgress)}%</span>
                </div>
                <ProgressBar ratio={stackProgress / 100} />
                <button
                  type="button"
                  onClick={() => void invoke("server_stack_upload_cancel")}
                  className={`${btnBase} mt-2 border border-rose-500/40 text-rose-200`}
                >
                  {tr("Отменить загрузку стека", "Cancel stack upload")}
                </button>
              </div>
            ) : null}
            {stackInfo ? (
              parsedStackInfo ? (
                <div className="mt-3 space-y-3 rounded-xl border border-white/10 bg-black/25 p-3">
                  <div className="grid gap-2 sm:grid-cols-2">
                    <div className="rounded-lg border border-white/10 bg-black/30 p-2">
                      <p className="text-[11px] uppercase tracking-wide text-slate-500">{t("auto.Dashboard_tsx.63")}</p>
                      <p className="mt-1 break-all text-sm text-slate-200">
                        {formatMaybe(parsedStackInfo.bundleVersion)}
                      </p>
                    </div>
                    <div className="rounded-lg border border-white/10 bg-black/30 p-2">
                      <p className="text-[11px] uppercase tracking-wide text-slate-500">{t("auto.Dashboard_tsx.64")}</p>
                      <p className="mt-1 break-all text-sm text-slate-200">
                        {formatMaybe(parsedStackInfo.deployServerBinaryVersion)}
                      </p>
                    </div>
                    <div className="rounded-lg border border-white/10 bg-black/30 p-2">
                      <p className="text-[11px] uppercase tracking-wide text-slate-500">{t("auto.Dashboard_tsx.65")}</p>
                      <p className="mt-1 text-sm text-slate-200">
                        {formatMaybe(parsedStackInfo.hostDashboardEnabled)}
                      </p>
                    </div>
                    <div className="rounded-lg border border-white/10 bg-black/30 p-2">
                      <p className="text-[11px] uppercase tracking-wide text-slate-500">{t("auto.Dashboard_tsx.66")}</p>
                      <p className="mt-1 text-sm text-slate-200">
                        {formatMaybe(parsedStackInfo.hostNginxPirateSite)}
                      </p>
                    </div>
                    <div className="rounded-lg border border-white/10 bg-black/30 p-2">
                      <p className="text-[11px] uppercase tracking-wide text-slate-500">
                        {t("auto.Dashboard_tsx.67")}
                      </p>
                      <p className="mt-1 break-all text-sm text-slate-200">
                        {formatMaybe(parsedStackInfo.hostGuiDetectedAtInstall)}
                      </p>
                    </div>
                    <div className="rounded-lg border border-white/10 bg-black/30 p-2">
                      <p className="text-[11px] uppercase tracking-wide text-slate-500">{t("auto.Dashboard_tsx.68")}</p>
                      <p className="mt-1 break-all text-sm text-slate-200">
                        {formatMaybe(parsedStackInfo.hostGuiInstallJson)}
                      </p>
                    </div>
                  </div>
                  {parsedManifest ? (
                    <div>
                      <p className="text-[11px] uppercase tracking-wide text-slate-500">{t("auto.Dashboard_tsx.69")}</p>
                      <div className="mt-1 max-h-44 overflow-auto rounded-lg bg-black/40 p-2">
                        {Object.entries(parsedManifest).map(([key, value]) => (
                          <div
                            key={key}
                            className="grid grid-cols-[minmax(110px,160px)_1fr] gap-2 border-b border-white/5 py-1 last:border-b-0"
                          >
                            <span className="text-xs text-slate-400">{key}</span>
                            <span className="break-all text-xs text-slate-200">{formatMaybe(value)}</span>
                          </div>
                        ))}
                      </div>
                    </div>
                  ) : parsedStackInfo.manifestJson ? (
                    <div>
                      <p className="text-[11px] uppercase tracking-wide text-slate-500">{t("auto.Dashboard_tsx.70")}</p>
                      <pre className="mt-1 max-h-32 overflow-auto rounded-lg bg-black/40 p-2 text-xs text-slate-400">
                        {parsedStackInfo.manifestJson}
                      </pre>
                    </div>
                  ) : null}
                </div>
              ) : (
                <pre className="mt-3 max-h-32 overflow-auto rounded-lg bg-black/40 p-2 text-xs text-slate-400">
                  {stackInfo}
                </pre>
              )
            ) : null}
            {stackMsg ? <p className="mt-3 text-sm text-slate-400">{stackMsg}</p> : null}

            <div className="mt-4 flex justify-end">
              <button
                type="button"
                onClick={() => setStackModalOpen(false)}
                className={`${btnBase} border border-white/10 bg-white/5`}
              >
                {t("auto.Dashboard_tsx.71")}
              </button>
            </div>
          </div>
        </ModalDialog>
      ) : null}

      {/* Confirm remove */}
      {removeId ? (
        <ModalDialog
          open
          zClassName="z-modalConfirm"
          onClose={() => setRemoveId(null)}
          panelClassName="w-full max-w-sm"
          role="alertdialog"
          aria-labelledby="remove-bookmark-title"
        >
          <div className="rounded-2xl border border-white/10 bg-surface p-6 shadow-2xl">
            <h3 id="remove-bookmark-title" className="font-semibold text-slate-100">
              {t("auto.Dashboard_tsx.72")}
            </h3>
            <p className="mt-2 text-sm text-slate-400">{t("auto.Dashboard_tsx.73")}</p>
            {removeBookmarkEntry ? (
              <div className="mt-3 rounded-lg border border-white/10 bg-black/25 px-3 py-2 text-left">
                <p className="truncate text-sm font-medium text-slate-200">{removeBookmarkEntry.label}</p>
                <code className="mt-1 block break-all text-xs text-orange-200/80">{removeBookmarkEntry.url}</code>
              </div>
            ) : null}
            <div className="mt-4 flex justify-end gap-2">
              <button
                type="button"
                data-modal-initial-focus
                onClick={() => setRemoveId(null)}
                className={`${btnBase} border border-white/10 bg-white/5`}
              >
                {t("auto.Dashboard_tsx.74")}
              </button>
              <button
                type="button"
                onClick={() => void onRemoveBookmark()}
                className={`${btnBase} bg-rose-600 text-white hover:bg-rose-500`}
              >
                {t("auto.Dashboard_tsx.75")}
              </button>
            </div>
          </div>
        </ModalDialog>
      ) : null}
    </>
  );
}

