/**
 * Guided project flow: folder → scan/preflight → build/test → deploy.
 * Main column = linear pipeline; right column = logs, rollback, toolchain.
 */
import { invoke, isTauri } from "@tauri-apps/api/core";
import { listen } from "@tauri-apps/api/event";
import {
  AlertCircle,
  Check,
  ChevronDown,
  ChevronRight,
  Globe,
  FileCode,
  FolderOpen,
  Loader2,
  Lock,
  Play,
  RotateCcw,
  Scan,
  ShieldCheck,
  Square,
  TestTube,
  Trash2,
  Wrench,
  X,
} from "lucide-react";
import React, { useCallback, useEffect, useRef, useState } from "react";
import { toast } from "sonner";
import type { HostServicesCompatSummary, ProjectsPreflightReport } from "./projects-preflight-types";
import { useCmdVarsPrompt } from "./useCmdVarsPrompt";
import type { ToolchainReport } from "./toolchain-types";
import { LocalToolchainPanel } from "./LocalToolchainPanel";
import { useIntervalWhenVisible } from "./hooks/useIntervalWhenVisible";
import { useI18n } from "./i18n";

const btnBase =
  "inline-flex items-center justify-center gap-2 rounded-lg px-4 py-2 text-sm font-semibold transition focus-visible:outline-none focus-visible:ring-2 focus-visible:ring-red-600/60 focus-visible:ring-offset-2 focus-visible:ring-offset-[#050204] disabled:pointer-events-none disabled:opacity-50";

const btnPrimary = `${btnBase} bg-gradient-to-r from-red-900 to-red-800 text-white shadow-glow hover:from-red-800 hover:to-red-700`;

/** Cap for in-memory local dev process log lines ([start].cmd stdout/stderr). */
const MAX_LOCAL_DEV_LOG_LINES = 2000;

/** User feedback: deploy/rollback/paas text stays inline in the right column; preflight and host-service install use toasts. */

type GuidedStep = 1 | 2 | 3 | 4;

type LocalDevLogEntry = { stream: "stdout" | "stderr"; line: string };
type ProjectDeployCheck = {
  projectId: string;
  uploaded: boolean;
  currentVersion: string;
  state: string;
};
type DetectedService = {
  name: string;
  port: number;
  type: string;
  source: string;
  confidence: number;
};
type NetworkAccessAnalysis = {
  projectId: string;
  detection: { services: DetectedService[]; warnings: string[] };
  nginxPreview?: string | null;
  hostServices: HostServicesCompatSummary;
  localNginxTemplateSha256?: string | null;
};
type DeployValidationReport = {
  allow: boolean;
  blockers: string[];
  warnings: string[];
  nginxState?: {
    serverTemplateSha256: string;
    appliedContentSha256: string;
    appliedSitePath: string;
    lastAppliedAtMs: number;
    needsUpdate: boolean;
  } | null;
};
type AccessMode = "local" | "lan" | "public";
type RouteRow = { path: string; target: string; websocket: boolean };

type LoadProjectNetworkManifestView = {
  accessMode: string;
  domain: string;
  domains: string[];
  routes: { path: string; target: string; websocket?: boolean }[];
  httpsEnabled: boolean;
  staticNginxFront: boolean;
};

function formatServerNginxFetchErr(raw: string, language: string): string {
  const m = raw.toLowerCase();
  if (language === "ru") {
    if (m.includes("control-api base url is not set")) {
      return `${raw}\n\nПодключитесь к deploy-server (GetStatus подставит HTTP base) или задайте control-api URL в настройках закладки.`;
    }
    if (m.includes("401") || m.includes("sign in") || m.includes("session expired")) {
      return `${raw}\n\nВойдите в разделе «Проекты на сервере» (JWT для control-api).`;
    }
  } else {
    if (m.includes("control-api base url is not set")) {
      return `${raw}\n\nConnect to deploy-server (GetStatus sets the HTTP base) or set the control-api URL in bookmark settings.`;
    }
    if (m.includes("401") || m.includes("sign in") || m.includes("session expired")) {
      return `${raw}\n\nSign in under Server projects (JWT for control-api).`;
    }
  }
  return raw;
}

function ProgressBarMini({ ratio }: { ratio: number }) {
  const w = Math.min(100, Math.max(0, ratio * 100));
  return (
    <div className="h-1.5 w-full overflow-hidden rounded-full bg-black/40">
      <div className="h-full rounded-full bg-gradient-to-r from-red-700 to-orange-600" style={{ width: `${w}%` }} />
    </div>
  );
}

function StepDot({
  n,
  label,
  current,
  done,
  onSelect,
}: {
  n: number;
  label: string;
  current: boolean;
  done: boolean;
  onSelect: () => void;
}) {
  return (
    <button
      type="button"
      onClick={onSelect}
      className={`flex min-w-0 flex-1 items-center gap-2 rounded-lg border px-2 py-2 text-left text-xs font-medium transition ${
        current
          ? "border-red-600/50 bg-red-950/45 text-red-50 shadow-[0_0_16px_rgba(220,38,38,0.15)]"
          : done
            ? "border-orange-800/40 bg-orange-950/25 text-orange-200/95"
            : "border-border-subtle bg-panel-raised text-slate-500 hover:border-white/15"
      }`}
    >
      <span
        className={`flex h-6 w-6 shrink-0 items-center justify-center rounded-full text-[11px] font-bold ${
          current ? "bg-gradient-to-r from-red-700 to-orange-600 text-white" : done ? "bg-orange-700/90 text-white" : "bg-slate-800 text-slate-400"
        }`}
      >
        {done && !current ? "✓" : n}
      </span>
      <span className="truncate">{label}</span>
    </button>
  );
}

const DEPLOY_PHASE_ORDER = ["prepare", "archive", "upload", "apply"] as const;

/**
 * One editable "public domain" input row. Memoized so typing in row N only
 * re-renders row N — not the whole 2k-line ProjectsPanel and every other row.
 * All handlers passed in are stable `useCallback`s keyed by index.
 */
const PublicDomainRow = React.memo(function PublicDomainRow({
  value,
  index,
  canRemove,
  onChange,
  onRemove,
}: {
  value: string;
  index: number;
  canRemove: boolean;
  onChange: (index: number, value: string) => void;
  onRemove: (index: number) => void;
}) {
  const { t } = useI18n();
  return (
    <div className="flex gap-2">
      <input
        value={value}
        onChange={(e) => onChange(index, e.target.value)}
        placeholder={index === 0 ? "shop.example.com" : "api.example.com"}
        className="min-w-0 flex-1 rounded border border-border-subtle bg-black/30 px-2 py-1.5 text-xs text-slate-100"
      />
      <button
        type="button"
        className="shrink-0 text-xs text-rose-300 hover:text-rose-200 disabled:opacity-40"
        disabled={!canRemove}
        onClick={() => onRemove(index)}
      >
        {t("auto.ProjectsPanel_tsx.83")}
      </button>
    </div>
  );
});

/** One editable route row (path / upstream / WS). Memoized — see above. */
const RouteEditorRow = React.memo(function RouteEditorRow({
  route,
  index,
  onPatch,
  onRemove,
}: {
  route: RouteRow;
  index: number;
  onPatch: (index: number, patch: Partial<RouteRow>) => void;
  onRemove: (index: number) => void;
}) {
  const { t, language } = useI18n();
  const tr = (ru: string, en: string) => (language === "ru" ? ru : en);
  return (
    <div className="grid gap-2 rounded border border-border-subtle bg-black/25 p-2 sm:grid-cols-[1fr_1fr_auto_auto]">
      <label className="block text-[11px] text-slate-500">
        {tr("Путь", "Path")}
        <input
          value={route.path}
          onChange={(e) => onPatch(index, { path: e.target.value })}
          placeholder="/api"
          className="mt-0.5 w-full rounded border border-border-subtle bg-black/30 px-2 py-1 text-xs text-slate-100"
        />
      </label>
      <label className="block text-[11px] text-slate-500">
        {tr("Upstream", "Upstream")}
        <input
          value={route.target}
          onChange={(e) => onPatch(index, { target: e.target.value })}
          placeholder="api:8080"
          className="mt-0.5 w-full rounded border border-border-subtle bg-black/30 px-2 py-1 text-xs text-slate-100"
        />
      </label>
      <label className="flex items-end gap-1.5 pb-1 text-xs text-slate-300">
        <input
          type="checkbox"
          checked={route.websocket}
          onChange={(e) => onPatch(index, { websocket: e.target.checked })}
        />
        WS
      </label>
      <button
        type="button"
        className="self-end pb-1 text-xs text-rose-300 hover:text-rose-200"
        onClick={() => onRemove(index)}
      >
        {t("auto.ProjectsPanel_tsx.83")}
      </button>
    </div>
  );
});

export function ProjectsPanel({
  deployDir,
  deployVersion,
  deploying,
  deployProgress,
  deployPhaseKey,
  deployPhaseDetail,
  deployMsg,
  deployCancelRequested,
  paasBusy,
  paasMsg,
  endpoint,
  onSetDeployVersion,
  onPickFolder,
  onDeploy,
  onDeployCancelRequest,
  onPipelineFull,
  onAfterRollback,
  runPaas,
  onSelectProjectPath,
  registryRefreshKey,
  onRegistryChanged,
  toolchainReport,
  toolchainLoading,
  toolchainErr,
  onRefreshToolchain,
}: {
  deployDir: string | null;
  deployVersion: string;
  deploying: boolean;
  deployProgress: number;
  /** Server-side deploy stage key (`prepare` … `apply`); only set while `deploying`. */
  deployPhaseKey: string | null;
  /** Optional line from desktop (upload session / retries / finalize). */
  deployPhaseDetail: string | null;
  deployMsg: string | null;
  deployCancelRequested: boolean;
  paasBusy: boolean;
  paasMsg: string | null;
  endpoint: string | null;
  onSetDeployVersion: (v: string) => void;
  onPickFolder: () => void;
  onDeploy: () => void;
  onDeployCancelRequest: () => void;
  onPipelineFull: () => void | Promise<void>;
  onAfterRollback?: () => void;
  runPaas: (
    label: string,
    fn: () => Promise<string | void>,
    opts?: { onSuccess?: () => void },
  ) => Promise<void>;
  onSelectProjectPath: (path: string) => void;
  registryRefreshKey: number;
  onRegistryChanged?: () => void;
  toolchainReport: ToolchainReport | null;
  toolchainLoading: boolean;
  toolchainErr: string | null;
  onRefreshToolchain: () => void;
}) {
  const { language, t } = useI18n();
  const tr = (ru: string, en: string) => (language === "ru" ? ru : en);
  const paasPath = deployDir;
  const { promptCmdVars, cmdVarsModal } = useCmdVarsPrompt(paasPath, language);
  const [guidedStep, setGuidedStep] = useState<GuidedStep>(1);
  const [advancedOpen, setAdvancedOpen] = useState(false);
  const [preflight, setPreflight] = useState<ProjectsPreflightReport | null>(null);
  const [preflightLoading, setPreflightLoading] = useState(false);
  const [rollbackVersion, setRollbackVersion] = useState("");
  const [rollbackBusy, setRollbackBusy] = useState(false);
  const [rollbackMsg, setRollbackMsg] = useState<string | null>(null);
  const [deleteCheck, setDeleteCheck] = useState<ProjectDeployCheck | null>(null);
  const [deleteBusy, setDeleteBusy] = useState(false);
  const [deleteConfirm, setDeleteConfirm] = useState(false);
  const [deleteMsg, setDeleteMsg] = useState<string | null>(null);
  const [networkBusy, setNetworkBusy] = useState(false);
  const [networkAnalysis, setNetworkAnalysis] = useState<NetworkAccessAnalysis | null>(null);
  const [networkValidation, setNetworkValidation] = useState<DeployValidationReport | null>(null);
  const [networkMsg, setNetworkMsg] = useState<string | null>(null);
  const [hostSvcInstallBusyId, setHostSvcInstallBusyId] = useState<string | null>(null);
  const [nginxConfigModalOpen, setNginxConfigModalOpen] = useState(false);
  const [envEditorOpen, setEnvEditorOpen] = useState(false);
  const [envFileContent, setEnvFileContent] = useState("");
  const [envFilePath, setEnvFilePath] = useState("");
  const [envSaving, setEnvSaving] = useState(false);
  const [envError, setEnvError] = useState<string | null>(null);
  const [controlApiBase, setControlApiBase] = useState<string | null>(null);
  const nginxModalLocalRefreshDoneRef = useRef(false);
  const [serverNginxContent, setServerNginxContent] = useState<string | null>(null);
  const [serverNginxPath, setServerNginxPath] = useState<string | null>(null);
  const [serverNginxLoading, setServerNginxLoading] = useState(false);
  const [serverNginxErr, setServerNginxErr] = useState<string | null>(null);
  const [accessMode, setAccessMode] = useState<AccessMode>("local");
  const [restrictAccess, setRestrictAccess] = useState(false);
  const [publicDomains, setPublicDomains] = useState<string[]>([""]);
  const [httpsEnabled, setHttpsEnabled] = useState(false);
  const [basicProtection, setBasicProtection] = useState(true);
  const [firewallOnlyRequired, setFirewallOnlyRequired] = useState(true);
  const [ipWhitelist, setIpWhitelist] = useState(false);
  const [privateRoutes, setPrivateRoutes] = useState(false);
  const [websocketSupport, setWebsocketSupport] = useState(true);
  const [stripPrefix, setStripPrefix] = useState(false);
  const [routeTimeout, setRouteTimeout] = useState("60");
  const [routeRows, setRouteRows] = useState<RouteRow[]>([]);
  const [staticNginxFront, setStaticNginxFront] = useState(false);

  // Stable index-keyed mutators so the memoized row components are not
  // invalidated on every parent re-render (state setters are stable → []).
  const updatePublicDomain = useCallback((index: number, value: string) => {
    setPublicDomains((prev) => prev.map((row, i) => (i === index ? value : row)));
  }, []);
  const removePublicDomain = useCallback((index: number) => {
    setPublicDomains((prev) =>
      prev.length <= 1 ? prev : prev.filter((_, i) => i !== index),
    );
  }, []);
  const patchRouteRow = useCallback(
    (index: number, patch: Partial<RouteRow>) => {
      setRouteRows((prev) =>
        prev.map((row, i) => (i === index ? { ...row, ...patch } : row)),
      );
    },
    [],
  );
  const removeRouteRow = useCallback((index: number) => {
    setRouteRows((prev) => prev.filter((_, i) => i !== index));
  }, []);

  const [localDevRunning, setLocalDevRunning] = useState(false);
  const [localDevPath, setLocalDevPath] = useState<string | null>(null);
  const [localDevBusy, setLocalDevBusy] = useState(false);
  const [localDevMsg, setLocalDevMsg] = useState<string | null>(null);
  const [localDevLogs, setLocalDevLogs] = useState<LocalDevLogEntry[]>([]);
  const localDevLogEndRef = useRef<HTMLDivElement>(null);

  const refreshLocalDev = useCallback(async () => {
    try {
      const s = await invoke<{ running: boolean; path: string | null }>("local_dev_status");
      setLocalDevRunning(s.running);
      setLocalDevPath(s.path ?? null);
    } catch {
      setLocalDevRunning(false);
      setLocalDevPath(null);
    }
  }, []);

  useEffect(() => {
    void refreshLocalDev();
  }, [refreshLocalDev, deployDir, guidedStep]);

  useIntervalWhenVisible(() => void refreshLocalDev(), 4000, guidedStep === 3);

  useEffect(() => {
    if (!isTauri()) return;
    let cancelled = false;
    let unlisten: (() => void) | undefined;
    void listen<LocalDevLogEntry>("local-dev-log", (event) => {
      const p = event.payload;
      setLocalDevLogs((prev) => {
        const next = [...prev, { stream: p.stream, line: p.line }];
        if (next.length > MAX_LOCAL_DEV_LOG_LINES) {
          return next.slice(-MAX_LOCAL_DEV_LOG_LINES);
        }
        return next;
      });
    }).then((fn) => {
      if (cancelled) fn();
      else unlisten = fn;
    });
    return () => {
      cancelled = true;
      unlisten?.();
    };
  }, []);

  useEffect(() => {
    localDevLogEndRef.current?.scrollIntoView({ behavior: "smooth" });
  }, [localDevLogs.length]);

  const runPreflight = useCallback(async () => {
    if (!deployDir?.trim()) {
      setPreflight(null);
      return;
    }
    setPreflightLoading(true);
    try {
      const raw = await invoke<string>("projects_preflight", {
        directory: deployDir,
        version: deployVersion.trim() || "v0",
      });
      const report = JSON.parse(raw) as ProjectsPreflightReport;
      setPreflight(report);
      if (report.ready) {
        toast.success(t("auto.ProjectsPanel_tsx.1"), { description: t("auto.ProjectsPanel_tsx.2") });
      } else {
        toast.warning(t("auto.ProjectsPanel_tsx.3"), { description: t("auto.ProjectsPanel_tsx.4") });
      }
    } catch (e) {
      setPreflight(null);
      toast.error(t("auto.ProjectsPanel_tsx.5"), { description: String(e) });
    } finally {
      setPreflightLoading(false);
    }
  }, [deployDir, deployVersion, t]);

  const applyPreflightFix = useCallback(
    async (fixId: string) => {
      if (!deployDir?.trim()) return;
      try {
        const msg = await invoke<string>("apply_manifest_fix", {
          directory: deployDir,
          fixId,
        });
        toast.success(tr("Исправление применено", "Fix applied"), { description: msg });
        await runPreflight();
        const v = await invoke<LoadProjectNetworkManifestView>("load_project_network_manifest", {
          directory: deployDir,
        });
        setStaticNginxFront(Boolean(v.staticNginxFront));
      } catch (e) {
        toast.error(tr("Не удалось применить исправление", "Failed to apply fix"), {
          description: String(e),
        });
      }
    },
    [deployDir, runPreflight, tr],
  );

  useEffect(() => {
    let cancelled = false;
    if (!deployDir?.trim() || !endpoint) {
      setDeleteCheck(null);
      setDeleteConfirm(false);
      setDeleteMsg(null);
      return;
    }
    void (async () => {
      try {
        const check = await invoke<ProjectDeployCheck>("check_project_uploaded", {
          directory: deployDir,
        });
        if (!cancelled) {
          setDeleteCheck(check);
        }
      } catch {
        if (!cancelled) {
          setDeleteCheck(null);
        }
      }
    })();
    return () => {
      cancelled = true;
    };
  }, [deployDir, endpoint]);

  const refreshNetworkAccess = useCallback(async (): Promise<boolean> => {
    if (!deployDir?.trim()) {
      setNetworkAnalysis(null);
      setNetworkValidation(null);
      return false;
    }
    setNetworkBusy(true);
    setNetworkMsg(null);
    try {
      const overrides: {
        domains?: string[];
        routes?: { path: string; target: string; websocket: boolean }[];
      } = {};
      const hosts =
        accessMode === "public" ? publicDomains.map((d) => d.trim()).filter(Boolean) : [];
      if (hosts.length > 0) {
        overrides.domains = hosts;
      }
      if (routeRows.length > 0) {
        overrides.routes = routeRows.map((r) => ({
          path: r.path,
          target: r.target,
          websocket: r.websocket,
        }));
      }
      const hasOverrides = Boolean(
        (overrides.domains && overrides.domains.length > 0) ||
          (overrides.routes && overrides.routes.length > 0),
      );
      const analysis = await invoke<NetworkAccessAnalysis>("analyze_network_access", {
        directory: deployDir,
        ...(hasOverrides ? { overrides } : {}),
      });
      setNetworkAnalysis(analysis);
      if (endpoint) {
        const raw = await invoke<string>("validate_network_access", {
          directory: deployDir,
        });
        setNetworkValidation(JSON.parse(raw) as DeployValidationReport);
      } else {
        setNetworkValidation(null);
      }
      return true;
    } catch (e) {
      setNetworkMsg(String(e));
      setNetworkValidation(null);
      setNetworkAnalysis(null);
      return false;
    } finally {
      setNetworkBusy(false);
    }
  }, [deployDir, endpoint, accessMode, publicDomains, routeRows]);

  useEffect(() => {
    if (!deployDir?.trim()) {
      setPublicDomains([""]);
      return;
    }
    let cancelled = false;
    void invoke<LoadProjectNetworkManifestView>("load_project_network_manifest", {
      directory: deployDir,
    })
      .then((v) => {
        if (cancelled) return;
        const mode = v.accessMode;
        if (mode === "public" || mode === "lan" || mode === "local") {
          setAccessMode(mode);
        }
        const hosts = [v.domain, ...(v.domains ?? [])].map((d) => d.trim()).filter(Boolean);
        setPublicDomains(hosts.length > 0 ? hosts : [""]);
        if (v.routes.length > 0) {
          setRouteRows(
            v.routes.map((r) => ({
              path: r.path,
              target: r.target,
              websocket: Boolean(r.websocket),
            })),
          );
        }
        setHttpsEnabled(v.httpsEnabled);
        setStaticNginxFront(Boolean(v.staticNginxFront));
      })
      .catch(() => {
        /* pirate.toml may be missing until first save */
      });
    return () => {
      cancelled = true;
    };
  }, [deployDir]);

  useEffect(() => {
    void invoke<string | null>("get_control_api_base")
      .then((b) => setControlApiBase(b?.trim() ? b : null))
      .catch(() => setControlApiBase(null));
  }, [endpoint, nginxConfigModalOpen]);

  useEffect(() => {
    if (!nginxConfigModalOpen) {
      nginxModalLocalRefreshDoneRef.current = false;
      return;
    }
    if (!deployDir?.trim()) return;
    if (networkAnalysis?.nginxPreview) return;
    if (nginxModalLocalRefreshDoneRef.current) return;
    nginxModalLocalRefreshDoneRef.current = true;
    void refreshNetworkAccess();
  }, [nginxConfigModalOpen, deployDir, networkAnalysis?.nginxPreview, refreshNetworkAccess]);

  const installMissingHostService = useCallback(
    async (id: string) => {
      const trimmed = id.trim();
      if (!trimmed) return;
      setHostSvcInstallBusyId(trimmed);
      try {
        const r = await invoke<string>("control_api_host_service_install", {
          id: trimmed,
          installEnvJson: JSON.stringify({ env: {} }),
        });
        toast.success(language === "ru" ? "Установка выполнена" : "Install finished", {
          description: r.length > 320 ? `${r.slice(0, 320)}…` : r,
        });
        await refreshNetworkAccess();
      } catch (e: unknown) {
        toast.error(language === "ru" ? "Ошибка установки" : "Install failed", {
          description: String(e),
        });
      } finally {
        setHostSvcInstallBusyId(null);
      }
    },
    [language, refreshNetworkAccess],
  );

  const onRegenerateNetworkConfig = useCallback(async () => {
    if (!deployDir?.trim()) return;
    const ok = await refreshNetworkAccess();
    if (ok) {
      toast.success(
        language === "ru" ? "Превью конфигурации обновлено." : "Proxy config preview refreshed.",
      );
    } else {
      toast.error(language === "ru" ? "Не удалось перегенерировать превью." : "Failed to regenerate preview.");
    }
  }, [deployDir, refreshNetworkAccess, language]);

  useEffect(() => {
    if (!nginxConfigModalOpen) return;
    const base = controlApiBase?.trim();
    if (!base) {
      setServerNginxLoading(false);
      setServerNginxErr(null);
      setServerNginxContent(null);
      setServerNginxPath(null);
      return;
    }
    let cancelled = false;
    setServerNginxLoading(true);
    setServerNginxErr(null);
    void invoke<string>("control_api_fetch_nginx_site_json")
      .then((raw) => {
        if (cancelled) return;
        try {
          const j = JSON.parse(raw) as { path?: string; content?: string };
          setServerNginxContent(j.content ?? "");
          setServerNginxPath(j.path ?? null);
        } catch {
          setServerNginxErr(language === "ru" ? "Некорректный ответ сервера." : "Invalid server response.");
          setServerNginxContent(null);
          setServerNginxPath(null);
        }
      })
      .catch((e: unknown) => {
        if (!cancelled) {
          setServerNginxErr(formatServerNginxFetchErr(String(e), language));
          setServerNginxContent(null);
          setServerNginxPath(null);
        }
      })
      .finally(() => {
        if (!cancelled) setServerNginxLoading(false);
      });
    return () => {
      cancelled = true;
    };
  }, [nginxConfigModalOpen, controlApiBase, language]);

  /** Latest refresh impl for effects that must not depend on `routeRows` / wizard fields (avoids analyze→setRouteRows→refresh loop). */
  const refreshNetworkAccessRef = useRef(refreshNetworkAccess);
  refreshNetworkAccessRef.current = refreshNetworkAccess;

  useEffect(() => {
    if (!deployDir?.trim()) {
      setNetworkAnalysis(null);
      setNetworkValidation(null);
      return;
    }
    void refreshNetworkAccessRef.current();
  }, [deployDir, endpoint]);

  useEffect(() => {
    if (routeRows.length > 0) return;
    const services = networkAnalysis?.detection.services ?? [];
    const web = services.find((s) => s.name === "web");
    const api = services.find((s) => s.name === "api");
    const nextRoutes: RouteRow[] = [];
    if (web) nextRoutes.push({ path: "/", target: `web:${web.port}`, websocket: false });
    if (api) nextRoutes.push({ path: "/api", target: `api:${api.port}`, websocket: true });
    setRouteRows((prev) => {
      if (
        prev.length === nextRoutes.length &&
        prev.every((p, i) => p.path === nextRoutes[i]?.path && p.target === nextRoutes[i]?.target)
      ) {
        return prev;
      }
      return nextRoutes;
    });
  }, [networkAnalysis, routeRows.length]);

  useEffect(() => {
    // Secure defaults for WAN/Public mode.
    if (accessMode === "public") {
      setHttpsEnabled(true);
      setBasicProtection(true);
      setFirewallOnlyRequired(true);
    }
  }, [accessMode]);

  useEffect(() => {
    if (!envEditorOpen || !deployDir?.trim()) return;
    setEnvError(null);
    invoke<{ env_path: string; content: string; exists: boolean }>("read_project_local_env", {
      directory: deployDir,
    })
      .then((v) => {
        setEnvFilePath(v.env_path);
        setEnvFileContent(v.content);
      })
      .catch((e) => setEnvError(String(e)));
  }, [envEditorOpen, deployDir]);

  async function saveEnvFile() {
    if (!deployDir?.trim()) return;
    setEnvSaving(true);
    setEnvError(null);
    try {
      await invoke("write_project_local_env", { directory: deployDir, content: envFileContent });
      setEnvEditorOpen(false);
    } catch (e) {
      setEnvError(String(e));
    } finally {
      setEnvSaving(false);
    }
  }

  const onDeleteServerProject = async () => {
    if (!deleteCheck?.uploaded) return;
    if (!deleteConfirm) {
      setDeleteConfirm(true);
      return;
    }
    setDeleteBusy(true);
    setDeleteMsg(null);
    try {
      const r = await invoke<{
        status: string;
        projectId: string;
        removedRoot: string;
        removedDbRows: number;
        hadManagedProcess: boolean;
        verifiedListenPorts: number[];
        portsStillBusy: number[];
        removedDeployEventsRows: number;
        removedProjectSnapshotsRows: number;
      }>("remove_server_project", {
        projectId: deleteCheck.projectId,
      });
      const ports =
        r.verifiedListenPorts?.length > 0
          ? `, portsVerified=${r.verifiedListenPorts.join(",")}`
          : "";
      const msg = `OK: ${r.status}, project=${r.projectId}, dbRows=${r.removedDbRows} (events=${r.removedDeployEventsRows}, snapshots=${r.removedProjectSnapshotsRows}), stopped=${r.hadManagedProcess}${ports}`;
      setDeleteMsg(msg);
      setDeleteConfirm(false);
      toast.success(t("auto.ProjectsPanel_tsx.6"), { description: r.projectId });
      if (deployDir?.trim()) {
        await runPreflight();
      }
      const fresh = await invoke<ProjectDeployCheck>("check_project_uploaded", {
        directory: deployDir,
      });
      setDeleteCheck(fresh);
      onAfterRollback?.();
    } catch (e) {
      const msg = String(e);
      setDeleteMsg(msg);
      toast.error(t("auto.ProjectsPanel_tsx.7"), { description: msg });
    } finally {
      setDeleteBusy(false);
    }
  };

  const publicHosts = useCallback(
    () => publicDomains.map((d) => d.trim()).filter(Boolean),
    [publicDomains],
  );

  const networkManifestInput = useCallback(() => {
    const hosts = publicHosts();
    return {
      accessMode,
      domain: hosts[0] ?? "",
      domains: hosts,
      routes: routeRows.map((r) => ({
        path: r.path,
        target: r.target,
        websocket: r.websocket,
      })),
      httpsEnabled,
    };
  }, [accessMode, publicHosts, routeRows, httpsEnabled]);

  const onApplyProjectNginx = async () => {
    if (!deployDir?.trim()) return;
    if (accessMode === "public" && publicHosts().length === 0) {
      toast.error(
        language === "ru" ? "Укажите хотя бы один домен." : "Add at least one public domain.",
      );
      return;
    }
    if (!staticNginxFront && routeRows.length === 0) {
      toast.error(
        language === "ru"
          ? "Добавьте хотя бы один маршрут (web/api)."
          : "Add at least one route (web/api).",
      );
      return;
    }
    setNetworkBusy(true);
    setNetworkMsg(null);
    try {
      await invoke<string>("ensure_deploy_project_id_for_deploy", { path: deployDir });
      await invoke("save_project_network_manifest", {
        directory: deployDir,
        input: networkManifestInput(),
      });
      const msg = await invoke<string>("control_api_apply_project_nginx", {
        directory: deployDir,
      });
      toast.success(
        language === "ru" ? "Nginx проекта применён на сервере" : "Project nginx applied on server",
        { description: msg.length > 400 ? `${msg.slice(0, 400)}…` : msg },
      );
      await refreshNetworkAccess();
    } catch (e) {
      const msg = String(e);
      setNetworkMsg(msg);
      toast.error(
        language === "ru" ? "Не удалось применить nginx проекта" : "Failed to apply project nginx",
        { description: msg },
      );
    } finally {
      setNetworkBusy(false);
    }
  };

  const onSetupDashboardNginx = async () => {
    setNetworkMsg(null);
    try {
      const hasWeb = (networkAnalysis?.detection.services ?? []).some((s) => s.name === "web");
      await invoke("control_api_ensure_nginx", {
        mode: hasWeb ? "with_ui" : "api_only",
      });
      toast.success(t("auto.ProjectsPanel_tsx.8"));
      await refreshNetworkAccess();
    } catch (e) {
      const msg = String(e);
      setNetworkMsg(msg);
      toast.error(t("auto.ProjectsPanel_tsx.9"), { description: msg });
    }
  };

  const onRollback = async () => {
    const v = rollbackVersion.trim();
    if (!v) {
      setRollbackMsg(t("auto.ProjectsPanel_tsx.10"));
      toast.error(t("auto.ProjectsPanel_tsx.11"));
      return;
    }
    setRollbackBusy(true);
    setRollbackMsg(null);
    try {
      if (deployDir?.trim()) {
        await invoke<string>("ensure_deploy_project_id_for_deploy", { path: deployDir });
      }
      const r = await invoke<{ status: string; activeVersion: string }>("rollback_deploy", {
        version: v,
      });
      setRollbackMsg(`OK: ${r.status} -> ${t("auto.ProjectsPanel_tsx.12")} ${r.activeVersion}`);
      toast.success(t("auto.ProjectsPanel_tsx.13"), { description: r.activeVersion });
      onAfterRollback?.();
    } catch (e) {
      const msg = String(e);
      setRollbackMsg(msg);
      toast.error(t("auto.ProjectsPanel_tsx.14"), { description: msg });
    } finally {
      setRollbackBusy(false);
    }
  };

  const stepLabels: { n: GuidedStep; label: string }[] = [
    { n: 1, label: t("auto.ProjectsPanel_tsx.15") },
    { n: 2, label: t("auto.ProjectsPanel_tsx.16") },
    { n: 3, label: t("auto.ProjectsPanel_tsx.17") },
    { n: 4, label: t("auto.ProjectsPanel_tsx.18") },
  ];
  const buildOutputCheck = preflight?.checks.find((c) => c.id === "build_output");
  const blockDeployByOutput = buildOutputCheck ? !buildOutputCheck.ok : false;
  const blockDeployByNetwork = (networkValidation?.blockers?.length ?? 0) > 0;
  const networkStatus: "local" | "public-secured" | "misconfigured" =
    networkValidation?.blockers?.length
      ? "misconfigured"
      : accessMode === "public" && httpsEnabled
        ? "public-secured"
        : "local";

  const stepper = (
    <div className="mb-6 flex flex-wrap gap-2">
      {stepLabels.map(({ n, label }) => (
        <StepDot
          key={n}
          n={n}
          label={label}
          current={guidedStep === n}
          done={guidedStep > n}
          onSelect={() => setGuidedStep(n)}
        />
      ))}
    </div>
  );

  const contextProject = (
    <div className="border-b border-border-subtle px-3 py-3">
      <p className="text-[10px] font-semibold uppercase tracking-wider text-slate-500">{t("auto.ProjectsPanel_tsx.19")}</p>
      {deployDir ? (
        <p className="mt-1 break-all font-mono text-xs text-slate-300">{deployDir}</p>
      ) : (
        <p className="mt-1 text-xs text-slate-500">{t("auto.ProjectsPanel_tsx.20")}</p>
      )}
      <p className="mt-2 text-[11px] text-slate-500">
        {t("auto.ProjectsPanel_tsx.21")}: <code className="text-slate-300">{deployVersion.trim() || "—"}</code>
      </p>
    </div>
  );

  const contextPreflight =
    preflight || preflightLoading ? (
      <div className="border-b border-border-subtle px-3 py-3">
        <p className="text-[10px] font-semibold uppercase tracking-wider text-slate-500">Preflight</p>
        {preflightLoading ? (
          <p className="mt-2 flex items-center gap-2 text-xs text-slate-400">
            <Loader2 className="h-3.5 w-3.5 animate-spin" />
            {t("auto.ProjectsPanel_tsx.22")}
          </p>
        ) : preflight ? (
          <ul className="mt-2 max-h-52 space-y-1.5 overflow-y-auto text-xs">
            {preflight.checks.map((c, idx) => (
              <li
                key={`${c.id}-${c.field ?? ""}-${c.fixId ?? ""}-${idx}`}
                className={`flex flex-col gap-1 ${c.ok ? "text-slate-300" : "text-rose-300"}`}
              >
                <div className="flex gap-2">
                  <span className="shrink-0">{c.ok ? "✓" : "✗"}</span>
                  <span>
                    <span className="font-medium text-slate-200">{c.title}</span>
                    {c.field ? (
                      <span className="ml-1 font-mono text-[10px] text-slate-500">{c.field}</span>
                    ) : null}
                    <span className="text-slate-500"> — {c.detail}</span>
                    {c.hint ? (
                      <span className="block text-[10px] text-orange-200/90">↳ {c.hint}</span>
                    ) : null}
                  </span>
                </div>
                {c.fixId && c.fixLabel ? (
                  <button
                    type="button"
                    onClick={() => void applyPreflightFix(c.fixId!)}
                    className="ml-5 w-fit rounded border border-amber-700/50 bg-amber-950/30 px-2 py-0.5 text-[10px] text-amber-100 hover:bg-amber-900/40"
                  >
                    {tr("Применить исправление", "Apply fix")}: {c.fixLabel}
                  </button>
                ) : null}
              </li>
            ))}
          </ul>
        ) : null}
      </div>
    ) : null;

  const contextLocalLogs = (
    <div className="flex min-h-0 flex-1 flex-col border-b border-border-subtle">
      <div className="flex items-center justify-between gap-2 px-3 py-2">
        <span className="text-[10px] font-semibold uppercase tracking-wider text-slate-500">
          {t("auto.ProjectsPanel_tsx.23")}
        </span>
        <div className="flex gap-1">
          <button
            type="button"
            onClick={() => {
              void navigator.clipboard.writeText(
                localDevLogs.map((l) => `[${l.stream}] ${l.line}`).join("\n"),
              );
              toast.message(t("auto.ProjectsPanel_tsx.24"));
            }}
            disabled={localDevLogs.length === 0}
            className="rounded px-2 py-0.5 text-[10px] text-slate-400 hover:bg-white/10 disabled:opacity-40"
          >
            {t("auto.ProjectsPanel_tsx.25")}
          </button>
          <button
            type="button"
            onClick={() => setLocalDevLogs([])}
            disabled={localDevLogs.length === 0}
            className="rounded px-2 py-0.5 text-[10px] text-slate-400 hover:bg-white/10 disabled:opacity-40"
          >
            {t("auto.ProjectsPanel_tsx.26")}
          </button>
        </div>
      </div>
      <pre
        className="min-h-[6rem] flex-1 overflow-y-auto whitespace-pre-wrap break-all px-3 pb-3 font-mono text-[10px] leading-relaxed text-slate-400"
        aria-live="polite"
      >
        {localDevMsg ? (
          <span className="text-rose-300">
            {localDevMsg}
            {"\n"}
          </span>
        ) : null}
        {localDevLogs.map((l, i) => (
          <span
            key={`${i}-${l.line.slice(0, 24)}`}
            className={`mb-0.5 block border-l-2 pl-2 ${
              l.stream === "stderr"
                ? "border-rose-500/50 text-rose-200/90"
                : "border-white/10 text-slate-400"
            }`}
          >
            {l.stream === "stderr" ? "[err] " : "[out] "}
            {l.line}
            {"\n"}
          </span>
        ))}
        <div ref={localDevLogEndRef} />
      </pre>
    </div>
  );

  const deployPhaseLabel = (key: string) => {
    switch (key) {
      case "prepare":
        return tr("Подготовка", "Prepare");
      case "archive":
        return tr("Архивация", "Archive");
      case "upload":
        return tr("Отправка", "Upload");
      case "apply":
        return tr("Запуск на сервере", "Apply on server");
      default:
        return key;
    }
  };

  const deployPhaseIndex = deployPhaseKey
    ? DEPLOY_PHASE_ORDER.indexOf(deployPhaseKey as (typeof DEPLOY_PHASE_ORDER)[number])
    : -1;

  const contextDeploy = (
    <div className="border-b border-border-subtle px-3 py-3">
      <p className="text-[10px] font-semibold uppercase tracking-wider text-slate-500">{t("auto.ProjectsPanel_tsx.27")}</p>
      {deploying ? (
        <div className="mt-2 space-y-2">
          <div className="mb-1 flex justify-between text-[11px] text-slate-500">
            <span className="text-slate-300">
              {deployPhaseKey ? deployPhaseLabel(deployPhaseKey) : t("auto.ProjectsPanel_tsx.28")}
            </span>
            <span>{Math.round(deployProgress)}%</span>
          </div>
          {deployPhaseDetail ? (
            <p className="text-[10px] leading-snug text-slate-500" title={deployPhaseDetail}>
              {deployPhaseDetail}
            </p>
          ) : null}
          <ProgressBarMini ratio={deployProgress / 100} />
          <ol className="flex flex-wrap gap-1.5" aria-label={tr("Этапы деплоя", "Deploy stages")}>
            {DEPLOY_PHASE_ORDER.map((k, i) => {
              const done = deployPhaseIndex > i;
              const current = deployPhaseIndex === i;
              return (
                <li
                  key={k}
                  className={`rounded-md border px-2 py-0.5 text-[10px] font-medium ${
                    current
                      ? "border-red-600/55 bg-red-950/40 text-red-100"
                      : done
                        ? "border-emerald-800/40 bg-emerald-950/25 text-emerald-200/90"
                        : "border-border-subtle bg-black/20 text-slate-500"
                  }`}
                >
                  {done && !current ? "✓ " : null}
                  {deployPhaseLabel(k)}
                </li>
              );
            })}
          </ol>
        </div>
      ) : null}
      {deployMsg ? <p className="mt-2 font-mono text-[11px] text-slate-400">{deployMsg}</p> : null}
      {deployCancelRequested ? (
        <p className="mt-1 text-[10px] text-orange-200/85">{t("auto.ProjectsPanel_tsx.29")}</p>
      ) : null}
      {paasMsg ? (
        <pre className="mt-2 max-h-32 overflow-auto whitespace-pre-wrap break-all font-mono text-[10px] text-slate-500">
          {paasMsg}
        </pre>
      ) : null}
    </div>
  );

  const contextRollback = (
    <div className="border-b border-border-subtle px-3 py-3">
      <p className="text-[10px] font-semibold uppercase tracking-wider text-slate-500">{t("auto.ProjectsPanel_tsx.30")}</p>
      <p className="mt-1 text-[11px] text-slate-500">
        {t("auto.ProjectsPanel_tsx.31")}
        <code className="text-slate-400">rollback</code>).
      </p>
      <div className="mt-2 flex gap-2">
        <input
          value={rollbackVersion}
          onChange={(e) => setRollbackVersion(e.target.value)}
          placeholder="v1.0.0"
          className="min-w-0 flex-1 rounded-lg border border-border-subtle bg-black/30 px-2 py-1.5 font-mono text-xs text-slate-100"
        />
        <button
          type="button"
          disabled={rollbackBusy || !rollbackVersion.trim()}
          onClick={() => void onRollback()}
          className={`${btnBase} shrink-0 border border-rose-800/50 bg-rose-950/40 text-rose-100`}
        >
          {rollbackBusy ? <Loader2 className="h-4 w-4 animate-spin" /> : <RotateCcw className="h-4 w-4" />}
        </button>
      </div>
      {rollbackMsg ? <p className="mt-2 text-xs text-slate-400">{rollbackMsg}</p> : null}
    </div>
  );

  const mainColumn = (
    <div className="min-w-0 flex-1 overflow-y-auto px-4 py-4 md:px-6">
      <div className="mx-auto max-w-3xl">
        <h1 className="font-display text-2xl text-red-400/95 drop-shadow-[0_0_12px_rgba(220,38,38,0.35)]">
          {t("auto.ProjectsPanel_tsx.32")}
        </h1>
        <p className="mt-1 text-sm text-slate-500">
          {t("auto.ProjectsPanel_tsx.33")}
        </p>
        {stepper}

        {guidedStep === 1 ? (
          <div className="space-y-4 rounded-lg border border-border-subtle bg-panel p-4">
            <p className="text-sm text-slate-300">
              {t("auto.ProjectsPanel_tsx.34")}<code className="rounded bg-black/40 px-1 text-orange-200/90">pirate.toml</code>
              ).
            </p>
            <div className="flex flex-wrap gap-2">
              <button
                type="button"
                onClick={() => void onPickFolder()}
                className={`${btnBase} border border-border-subtle bg-panel-raised text-slate-200 hover:bg-white/[0.06]`}
              >
                <FolderOpen className="h-4 w-4" />
                {t("auto.ProjectsPanel_tsx.35")}
              </button>
              <button
                type="button"
                disabled={!paasPath || paasBusy}
                onClick={() =>
                  void runPaas("Сканирование", () =>
                    invoke<string>("paas_scan_project", {
                      path: paasPath!,
                      dryRun: false,
                    }),
                  )
                }
                className={`${btnBase} border border-red-900/45 bg-red-950/35 text-orange-100`}
              >
                <Scan className="h-4 w-4" />
                {t("auto.ProjectsPanel_tsx.36")}
              </button>
              <button
                type="button"
                disabled={!paasPath || paasBusy}
                onClick={() =>
                  void runPaas(
                    "Init project",
                    () =>
                      invoke<string>("paas_init_project", {
                        path: paasPath!,
                        name: null,
                      }),
                    { onSuccess: onRegistryChanged },
                  )
                }
                className={`${btnBase} border border-border-subtle bg-panel-raised text-slate-200`}
              >
                Init project
              </button>
            </div>
            {deployDir ? (
              <p className="break-all font-mono text-sm text-orange-300/90">
                <FolderOpen className="mr-1 inline h-4 w-4" />
                {deployDir}
              </p>
            ) : (
              <p className="text-sm text-slate-500">{t("auto.ProjectsPanel_tsx.37")}</p>
            )}
            <button type="button" disabled={!deployDir} onClick={() => setGuidedStep(2)} className={btnPrimary}>
              {t("auto.ProjectsPanel_tsx.38")}
              <ChevronRight className="h-4 w-4" />
            </button>
          </div>
        ) : null}

        {guidedStep === 2 ? (
          <div className="space-y-4 rounded-lg border border-border-subtle bg-panel p-4">
            <p className="text-sm text-slate-300">
              {t("auto.ProjectsPanel_tsx.39")}
            </p>
            <div className="flex flex-wrap gap-2">
              <button
                type="button"
                disabled={!paasPath || paasBusy}
                onClick={() =>
                  void runPaas("Сканирование", () =>
                    invoke<string>("paas_scan_project", { path: paasPath!, dryRun: false }),
                  )
                }
                className={`${btnBase} border border-border-subtle bg-panel-raised text-slate-200`}
              >
                <Scan className="h-4 w-4" />
                {t("auto.ProjectsPanel_tsx.40")}
              </button>
              <button
                type="button"
                disabled={preflightLoading || !deployDir}
                onClick={() => void runPreflight()}
                className={`${btnBase} border border-orange-900/40 bg-orange-950/30 text-orange-100`}
              >
                {preflightLoading ? <Loader2 className="h-4 w-4 animate-spin" /> : <Check className="h-4 w-4" />}
                {t("auto.ProjectsPanel_tsx.41")}
              </button>
            </div>
            <div className="flex flex-wrap gap-2">
              <button
                type="button"
                onClick={() => setGuidedStep(1)}
                className={`${btnBase} border border-border-subtle bg-panel-raised text-slate-300`}
              >
                {t("auto.ProjectsPanel_tsx.42")}
              </button>
              <button type="button" disabled={!deployDir} onClick={() => setGuidedStep(3)} className={btnPrimary}>
                {t("auto.ProjectsPanel_tsx.43")}
                <ChevronRight className="h-4 w-4" />
              </button>
            </div>
          </div>
        ) : null}

        {guidedStep === 3 ? (
          <div className="space-y-4 rounded-lg border border-border-subtle bg-panel p-4">
            <p className="text-sm text-slate-300">
              {t("auto.ProjectsPanel_tsx.44")}
            </p>
            <div className="rounded-lg border border-border-subtle bg-black/25 px-3 py-3">
              <p className="text-xs font-medium text-slate-400">{t("auto.ProjectsPanel_tsx.45")}</p>
              {localDevRunning &&
              deployDir &&
              localDevPath &&
              deployDir.replace(/\/$/, "") !== localDevPath.replace(/\/$/, "") ? (
                <p className="mt-2 text-xs text-orange-200/90">
                  {t("auto.ProjectsPanel_tsx.46")}{" "}
                  <code className="break-all text-orange-100">{localDevPath}</code>
                </p>
              ) : null}
              <div className="mt-3 flex flex-wrap gap-2">
                {localDevRunning ? (
                  <button
                    type="button"
                    disabled={localDevBusy}
                    onClick={() => {
                      void (async () => {
                        setLocalDevBusy(true);
                        setLocalDevMsg(null);
                        try {
                          await invoke("local_dev_stop");
                          await refreshLocalDev();
                        } catch (e) {
                          setLocalDevMsg(String(e));
                        } finally {
                          setLocalDevBusy(false);
                        }
                      })();
                    }}
                    className={`${btnBase} border border-rose-800/50 bg-rose-950/40 text-rose-100`}
                  >
                    {localDevBusy ? <Loader2 className="h-4 w-4 animate-spin" /> : <Square className="h-4 w-4" />}
                    {t("auto.ProjectsPanel_tsx.47")}
                  </button>
                ) : (
                  <button
                    type="button"
                    disabled={paasBusy || localDevBusy || !paasPath}
                    onClick={() => {
                      void (async () => {
                        if (!paasPath) return;
                        setLocalDevBusy(true);
                        setLocalDevMsg(null);
                        setLocalDevLogs([]);
                        try {
                          const vars = await promptCmdVars(
                            ["start"],
                            tr("Параметры запуска", "Start parameters"),
                          );
                          if (vars === null) return;
                          await invoke("local_dev_start", { path: paasPath, cmdVars: vars });
                          await refreshLocalDev();
                        } catch (e) {
                          setLocalDevMsg(String(e));
                        } finally {
                          setLocalDevBusy(false);
                        }
                      })();
                    }}
                    className={`${btnBase} border border-orange-900/40 bg-orange-950/30 text-orange-100`}
                  >
                    {localDevBusy ? <Loader2 className="h-4 w-4 animate-spin" /> : <Play className="h-4 w-4" />}
                    {t("auto.ProjectsPanel_tsx.48")}
                  </button>
                )}
              </div>
            </div>
            <div className="flex flex-wrap gap-2">
              <button
                type="button"
                disabled={paasBusy || !paasPath}
                onClick={() =>
                  void runPaas("Build", async () => {
                    const vars = await promptCmdVars(["build"], tr("Параметры сборки", "Build parameters"));
                    if (vars === null) return tr("Отменено", "Cancelled");
                    return invoke<string>("paas_project_build", { path: paasPath!, cmdVars: vars });
                  })
                }
                className={`${btnBase} border border-border-subtle bg-panel-raised text-slate-200`}
              >
                <Wrench className="h-4 w-4" />
                Build
              </button>
              <button
                type="button"
                disabled={paasBusy || !paasPath}
                onClick={() =>
                  void runPaas("Test", async () => {
                    const vars = await promptCmdVars(["test"], tr("Параметры теста", "Test parameters"));
                    if (vars === null) return tr("Отменено", "Cancelled");
                    return invoke<string>("paas_project_test", { path: paasPath!, cmdVars: vars });
                  })
                }
                className={`${btnBase} border border-border-subtle bg-panel-raised text-slate-200`}
              >
                <TestTube className="h-4 w-4" />
                Test
              </button>
              <button
                type="button"
                disabled={paasBusy || !paasPath}
                onClick={() =>
                  void runPaas("Test locally", () =>
                    invoke<string>("paas_test_local", { path: paasPath!, image: null }),
                  )
                }
                className={`${btnBase} border border-border-subtle bg-panel-raised text-slate-200`}
              >
                Test locally
              </button>
            </div>
            <div className="flex flex-wrap gap-2">
              <button
                type="button"
                onClick={() => setGuidedStep(2)}
                className={`${btnBase} border border-border-subtle bg-panel-raised text-slate-300`}
              >
                {t("auto.ProjectsPanel_tsx.49")}
              </button>
              <button type="button" onClick={() => setGuidedStep(4)} className={btnPrimary}>
                {t("auto.ProjectsPanel_tsx.50")}
                <ChevronRight className="h-4 w-4" />
              </button>
            </div>
          </div>
        ) : null}

        {guidedStep === 4 ? (
          <div className="space-y-4 rounded-lg border border-border-subtle bg-panel p-4">
            <p className="text-sm text-slate-300">
              {t("auto.ProjectsPanel_tsx.51")}
            </p>
            <label className="block text-xs font-medium text-slate-500">
              {t("auto.ProjectsPanel_tsx.52")}
              <input
                value={deployVersion}
                onChange={(e) => onSetDeployVersion(e.target.value)}
                className="mt-1 w-full rounded-lg border border-border-subtle bg-black/30 px-3 py-2 font-mono text-sm text-slate-100"
                placeholder="v1.2.0"
              />
            </label>
            <div className="flex flex-wrap gap-2">
              <button
                type="button"
                disabled={preflightLoading || !deployDir}
                onClick={() => void runPreflight()}
                className={`${btnBase} border border-orange-900/40 bg-orange-950/30 text-orange-100`}
              >
                {preflightLoading ? <Loader2 className="h-4 w-4 animate-spin" /> : <Check className="h-4 w-4" />}
                {t("auto.ProjectsPanel_tsx.53")}
              </button>
              <button
                type="button"
                disabled={
                  deploying ||
                  !deployDir ||
                  !deployVersion.trim() ||
                  !endpoint ||
                  blockDeployByOutput ||
                  blockDeployByNetwork
                }
                onClick={() => void onDeploy()}
                className={btnPrimary}
              >
                {deploying ? <Loader2 className="h-4 w-4 animate-spin" /> : <Play className="h-4 w-4" />}
                Deploy
              </button>
              {deploying ? (
                <button
                  type="button"
                  onClick={() => {
                    void invoke("deploy_upload_cancel");
                    onDeployCancelRequest();
                  }}
                  className={`${btnBase} border border-rose-800/50 text-rose-200`}
                >
                  <X className="h-4 w-4" />
                  {t("auto.ProjectsPanel_tsx.54")}
                </button>
              ) : null}
            </div>
            {!endpoint ? (
              <p className="flex items-center gap-2 text-sm text-orange-200/90">
                <AlertCircle className="h-4 w-4 shrink-0" />
                {t("auto.ProjectsPanel_tsx.55")}
              </p>
            ) : null}
            {blockDeployByOutput ? (
              <p className="flex items-center gap-2 text-sm text-rose-300">
                <AlertCircle className="h-4 w-4 shrink-0" />
                {buildOutputCheck?.detail}
              </p>
            ) : null}
            {blockDeployByNetwork ? (
              <p className="flex items-center gap-2 text-sm text-rose-300">
                <AlertCircle className="h-4 w-4 shrink-0" />
                {t("auto.ProjectsPanel_tsx.56")}
              </p>
            ) : null}
            <div className="flex flex-wrap gap-2 border-t border-border-subtle pt-4">
              <p className="w-full text-xs text-slate-500">{t("auto.ProjectsPanel_tsx.57")}</p>
              <button
                type="button"
                disabled={paasBusy || !deployDir || !deployVersion.trim()}
                onClick={() => void onPipelineFull()}
                className={`${btnBase} border border-red-900/50 bg-red-950/30 text-orange-100`}
              >
                {paasBusy ? <Loader2 className="h-4 w-4 animate-spin" /> : null}
                {t("auto.ProjectsPanel_tsx.58")}
              </button>
            </div>
            <div className="space-y-2 border-t border-border-subtle pt-4">
              <div className="flex items-center justify-between gap-2">
                <div>
                  <p className="text-xs font-semibold uppercase tracking-wide text-slate-500">{t("auto.ProjectsPanel_tsx.59")}</p>
                  <p className="mt-0.5 text-xs text-slate-500">{t("auto.ProjectsPanel_tsx.60")}</p>
                </div>
                <div className="flex items-center gap-2">
                  <span
                    className={`rounded-full border px-2 py-1 text-[11px] font-semibold ${
                      networkStatus === "misconfigured"
                        ? "border-rose-700/60 bg-rose-950/40 text-rose-200"
                        : networkStatus === "public-secured"
                          ? "border-emerald-700/60 bg-emerald-950/40 text-emerald-200"
                          : "border-orange-800/60 bg-orange-950/35 text-orange-200"
                    }`}
                  >
                    {networkStatus === "misconfigured"
                      ? t("auto.ProjectsPanel_tsx.61")
                      : networkStatus === "public-secured"
                        ? t("auto.ProjectsPanel_tsx.62")
                        : t("auto.ProjectsPanel_tsx.63")}
                  </span>
                  <button
                    type="button"
                    disabled={networkBusy || !deployDir}
                    onClick={() => void refreshNetworkAccess()}
                    className={`${btnBase} border border-border-subtle bg-panel-raised text-slate-200`}
                  >
                    {networkBusy ? <Loader2 className="h-4 w-4 animate-spin" /> : <Scan className="h-4 w-4" />}
                    {t("auto.ProjectsPanel_tsx.64")}
                  </button>
                </div>
              </div>
              <div className="rounded-lg border border-border-subtle bg-black/20 p-3">
                <p className="text-xs font-semibold uppercase tracking-wide text-slate-500">{t("auto.ProjectsPanel_tsx.65")}</p>
                {(networkAnalysis?.detection.services ?? []).length > 0 ? (
                  <div className="mt-2 grid gap-2">
                    {(networkAnalysis?.detection.services ?? []).map((s) => (
                      <div key={`${s.name}:${s.port}`} className="rounded-md border border-border-subtle bg-black/20 p-2">
                        <div className="flex items-center justify-between gap-2">
                          <p className="text-xs font-semibold text-slate-200">{s.name.toUpperCase()}</p>
                          <span className="text-[11px] text-emerald-300">{t("auto.ProjectsPanel_tsx.66")}</span>
                        </div>
                        <div className="mt-2 flex flex-wrap items-center gap-2 text-xs text-slate-400">
                          <span>{t("auto.ProjectsPanel_tsx.67")}: {s.source}</span>
                          <span>{t("auto.ProjectsPanel_tsx.68")}: {s.type}</span>
                          <label className="inline-flex items-center gap-1">
                            {t("auto.ProjectsPanel_tsx.69")}
                            <input
                              defaultValue={s.port}
                              className="w-20 rounded border border-border-subtle bg-black/30 px-2 py-1 text-xs text-slate-200"
                            />
                          </label>
                        </div>
                      </div>
                    ))}
                  </div>
                ) : (
                  <p className="mt-2 text-xs text-slate-500">{t("auto.ProjectsPanel_tsx.70")}</p>
                )}
              </div>
              <div className="rounded-lg border border-border-subtle bg-black/20 p-3">
                <p className="text-xs font-semibold uppercase tracking-wide text-slate-500">
                  {tr("Хост-сервисы (манифест vs сервер)", "Host services (manifest vs server)")}
                </p>
                {networkAnalysis?.hostServices ? (
                  <div className="mt-2 space-y-2 text-xs">
                    {networkAnalysis.hostServices.status === "none" ? (
                      <p className="text-slate-400">
                        {tr(
                          "Манифест не запрашивает отдельные пакеты на хосте ([services], [runtime], [proxy]).",
                          "Manifest does not require extra host packages ([services], [runtime], [proxy]).",
                        )}
                      </p>
                    ) : null}
                    {networkAnalysis.hostServices.status === "skipped" ? (
                      <div className="rounded border border-amber-900/50 bg-amber-950/25 px-2 py-1.5 text-amber-100/95">
                        <p>
                          {tr("Требуются на хосте:", "Required on host:")}{" "}
                          <code className="text-amber-50">
                            {networkAnalysis.hostServices.requiredHostServiceIds.join(", ") || "—"}
                          </code>
                        </p>
                        <p className="mt-1 text-amber-200/80">
                          {networkAnalysis.hostServices.skipReason ||
                            tr(
                              "Войдите в control-api (закладка сервера), чтобы сравнить с установленными пакетами.",
                              "Sign in to control-api (server bookmark) to compare with installed packages.",
                            )}
                        </p>
                      </div>
                    ) : null}
                    {networkAnalysis.hostServices.status === "checked" ? (
                      <>
                        <p className="text-slate-400">
                          {tr("Запрошено:", "Requested:")}{" "}
                          <code className="text-slate-200">
                            {networkAnalysis.hostServices.requiredHostServiceIds.join(", ") || "—"}
                          </code>
                        </p>
                        {networkAnalysis.hostServices.missingHostServiceIds.length > 0 ? (
                          <div className="space-y-2">
                            <p className="text-rose-300">
                              {tr("Не установлены на сервере:", "Not installed on server:")}{" "}
                              <code className="text-rose-100">
                                {networkAnalysis.hostServices.missingHostServiceIds.join(", ")}
                              </code>
                            </p>
                            <div className="flex flex-wrap items-center gap-2">
                              {networkAnalysis.hostServices.missingHostServiceIds.map((svcId) => {
                                const canInstall =
                                  networkAnalysis.hostServices.dispatchScriptPresent !== false;
                                const busy = hostSvcInstallBusyId === svcId;
                                return (
                                  <button
                                    key={svcId}
                                    type="button"
                                    disabled={busy || networkBusy || !canInstall}
                                    onClick={() => void installMissingHostService(svcId)}
                                    className={
                                      canInstall
                                        ? "inline-flex items-center gap-1.5 rounded-lg border border-red-800/60 bg-red-950/45 px-2.5 py-1.5 text-xs font-semibold text-orange-100 transition hover:bg-red-950/65 disabled:opacity-50"
                                        : "inline-flex cursor-not-allowed items-center gap-1.5 rounded-lg border border-white/10 bg-black/25 px-2.5 py-1.5 text-xs font-semibold text-slate-500"
                                    }
                                  >
                                    {busy ? (
                                      <Loader2 className="h-3.5 w-3.5 shrink-0 animate-spin" />
                                    ) : null}
                                    {tr(`Установить «${svcId}»`, `Install ${svcId}`)}
                                  </button>
                                );
                              })}
                            </div>
                            {hostSvcInstallBusyId ? (
                              <div
                                className="space-y-2 rounded-lg border border-amber-900/35 bg-amber-950/20 px-2.5 py-2"
                                role="status"
                                aria-live="polite"
                                aria-busy="true"
                              >
                                <p className="text-xs text-amber-100/90">
                                  {tr("Установка сервиса", "Installing service")}:{" "}
                                  <code className="font-mono text-amber-200">{hostSvcInstallBusyId}</code>
                                </p>
                                <div className="host-svc-progress-track" aria-hidden>
                                  <div className="host-svc-progress-bar" />
                                </div>
                                <p className="text-[10px] text-slate-500">
                                  {tr(
                                    "Ожидание ответа сервера (sudo-скрипт). Это может занять несколько минут.",
                                    "Waiting for the server (sudo script). This may take several minutes.",
                                  )}
                                </p>
                              </div>
                            ) : null}
                            {networkAnalysis.hostServices.dispatchScriptPresent === false ? (
                              <p className="text-[11px] text-amber-200/90">
                                {tr(
                                  "Скрипт pirate-host-service.sh не найден на сервере — обновите стек (install/OTA), затем повторите.",
                                  "pirate-host-service.sh not on the host — update stack (install/OTA), then retry.",
                                )}
                              </p>
                            ) : null}
                          </div>
                        ) : (
                          <p className="text-emerald-300/95">
                            {tr(
                              "Все перечисленные сервисы присутствуют на хосте.",
                              "All listed services are present on the host.",
                            )}
                          </p>
                        )}
                        {networkAnalysis.hostServices.dispatchScriptPresent === false &&
                        networkAnalysis.hostServices.missingHostServiceIds.length === 0 ? (
                          <p className="text-amber-200/90">
                            {tr(
                              "Скрипт pirate-host-service.sh не найден на сервере (обновите install/OTA).",
                              "pirate-host-service.sh not found on the host (re-run install or OTA).",
                            )}
                          </p>
                        ) : null}
                      </>
                    ) : null}
                    {networkAnalysis.hostServices.status === "error" ? (
                      <p className="text-rose-300">
                        {networkAnalysis.hostServices.skipReason ||
                          tr("Не удалось запросить список сервисов.", "Could not fetch host services list.")}
                      </p>
                    ) : null}
                  </div>
                ) : (
                  <p className="mt-2 text-xs text-slate-500">
                    {tr("Запустите анализ сети.", "Run network analysis.")}
                  </p>
                )}
              </div>
              <div className="rounded-lg border border-border-subtle bg-black/20 p-3">
                <p className="text-xs font-semibold uppercase tracking-wide text-slate-500">{t("auto.ProjectsPanel_tsx.71")}</p>
                <div className="mt-2 flex flex-wrap gap-2">
                  {[
                    { id: "local" as const, label: t("auto.ProjectsPanel_tsx.72") },
                    { id: "lan" as const, label: t("auto.ProjectsPanel_tsx.73") },
                    { id: "public" as const, label: t("auto.ProjectsPanel_tsx.74") },
                  ].map((opt) => (
                    <button
                      key={opt.id}
                      type="button"
                      onClick={() => setAccessMode(opt.id)}
                      className={`rounded-md border px-3 py-1.5 text-xs ${
                        accessMode === opt.id
                          ? "border-red-700/70 bg-red-950/40 text-red-100"
                          : "border-border-subtle bg-panel-raised text-slate-300"
                      }`}
                    >
                      {opt.label}
                    </button>
                  ))}
                </div>
                {accessMode === "local" ? (
                  <p className="mt-2 text-xs text-slate-400">{t("auto.ProjectsPanel_tsx.75")}: http://localhost:3000</p>
                ) : null}
                {accessMode === "lan" ? (
                  <div className="mt-2 space-y-1 text-xs text-slate-400">
                    <p>{t("auto.ProjectsPanel_tsx.76")}</p>
                    <p>IP: 192.168.0.12</p>
                    <label className="inline-flex items-center gap-2 text-slate-300">
                      <input type="checkbox" checked={restrictAccess} onChange={(e) => setRestrictAccess(e.target.checked)} />
                      {t("auto.ProjectsPanel_tsx.77")}
                    </label>
                  </div>
                ) : null}
                {accessMode === "public" ? (
                  <div className="mt-3 space-y-3 rounded-md border border-red-900/40 bg-red-950/15 p-3">
                    <p className="text-xs font-semibold text-orange-100">{t("auto.ProjectsPanel_tsx.78")}</p>
                    <div>
                      <p className="text-xs font-medium text-slate-400">
                        {tr("Домены (nginx server_name)", "Domains (nginx server_name)")}
                      </p>
                      <div className="mt-2 space-y-2">
                        {publicDomains.map((d, idx) => (
                          <PublicDomainRow
                            key={`pub-dom-${idx}`}
                            value={d}
                            index={idx}
                            canRemove={publicDomains.length > 1}
                            onChange={updatePublicDomain}
                            onRemove={removePublicDomain}
                          />
                        ))}
                      </div>
                      <button
                        type="button"
                        className={`${btnBase} mt-2 border border-border-subtle bg-panel-raised text-slate-200`}
                        onClick={() => setPublicDomains((prev) => [...prev, ""])}
                      >
                        {tr("Добавить домен", "Add domain")}
                      </button>
                    </div>
                    {staticNginxFront ? (
                      <p className="text-[11px] text-slate-400">
                        {tr(
                          "Static SPA — nginx отдаёт dist из релиза; маршруты proxy не нужны (шаблон pirate-nginx-snippet.conf).",
                          "Static SPA — nginx serves dist from the release; proxy routes are not required (pirate-nginx-snippet.conf template).",
                        )}
                      </p>
                    ) : (
                    <div>
                      <p className="text-xs font-medium text-slate-400">{t("auto.ProjectsPanel_tsx.82")}</p>
                      <p className="mt-1 text-[11px] text-slate-500">
                        {tr(
                          "Путь → upstream (web:3000, api:8080). WS — WebSocket для /api, /ws и т.п.",
                          "Path → upstream (web:3000, api:8080). WS — WebSocket for /api, /ws, etc.",
                        )}
                      </p>
                      <div className="mt-2 space-y-2">
                        {routeRows.map((r, idx) => (
                          <RouteEditorRow
                            key={`route-${idx}`}
                            route={r}
                            index={idx}
                            onPatch={patchRouteRow}
                            onRemove={removeRouteRow}
                          />
                        ))}
                      </div>
                      <button
                        type="button"
                        className={`${btnBase} mt-2 border border-border-subtle bg-panel-raised text-slate-200`}
                        onClick={() =>
                          setRouteRows((prev) => [
                            ...prev,
                            { path: "/api", target: "api:8080", websocket: true },
                          ])
                        }
                      >
                        {t("auto.ProjectsPanel_tsx.84")}
                      </button>
                    </div>
                    )}
                    <label className="inline-flex items-center gap-2 text-xs text-slate-300">
                      <input type="checkbox" checked={httpsEnabled} onChange={(e) => setHttpsEnabled(e.target.checked)} />
                      {t("auto.ProjectsPanel_tsx.80")}
                    </label>
                    <label className="inline-flex items-center gap-2 text-xs text-slate-300">
                      <input type="checkbox" checked={basicProtection} onChange={(e) => setBasicProtection(e.target.checked)} />
                      {t("auto.ProjectsPanel_tsx.81")}
                    </label>
                  </div>
                ) : null}
              </div>
              {accessMode !== "public" && routeRows.length > 0 ? (
                <div className="rounded-lg border border-border-subtle bg-black/20 p-3">
                  <p className="text-xs font-semibold uppercase tracking-wide text-slate-500">{t("auto.ProjectsPanel_tsx.82")}</p>
                  <div className="mt-2 space-y-1 text-xs text-slate-300">
                    {routeRows.map((r, idx) => (
                      <div key={`${r.path}-${idx}`} className="flex items-center justify-between gap-2 rounded border border-border-subtle bg-black/20 px-2 py-1">
                        <span>
                          <code>{r.path}</code> → <code>{r.target}</code>
                        </span>
                        <button
                          type="button"
                          className="text-rose-300 hover:text-rose-200"
                          onClick={() => setRouteRows((prev) => prev.filter((_, i) => i !== idx))}
                        >
                          {t("auto.ProjectsPanel_tsx.83")}
                        </button>
                      </div>
                    ))}
                  </div>
                  <div className="mt-2 flex gap-2">
                    <button
                      type="button"
                      className={`${btnBase} border border-border-subtle bg-panel-raised text-slate-200`}
                      onClick={() => setRouteRows((prev) => [...prev, { path: "/new", target: "web:3000", websocket: false }])}
                    >
                      {t("auto.ProjectsPanel_tsx.84")}
                    </button>
                  </div>
                  <details className="mt-2 rounded border border-border-subtle bg-black/20 p-2 text-xs text-slate-300">
                    <summary className="cursor-pointer font-semibold text-slate-300">{t("auto.ProjectsPanel_tsx.85")}</summary>
                    <div className="mt-2 grid gap-2">
                      <label className="inline-flex items-center gap-2">
                        <input type="checkbox" checked={stripPrefix} onChange={(e) => setStripPrefix(e.target.checked)} />
                        {t("auto.ProjectsPanel_tsx.86")}
                      </label>
                      <label className="inline-flex items-center gap-2">
                        <input
                          type="checkbox"
                          checked={websocketSupport}
                          onChange={(e) => setWebsocketSupport(e.target.checked)}
                        />
                        {t("auto.ProjectsPanel_tsx.87")}
                      </label>
                      <label className="inline-flex items-center gap-2">
                        {t("auto.ProjectsPanel_tsx.88")}:
                        <input
                          value={routeTimeout}
                          onChange={(e) => setRouteTimeout(e.target.value)}
                          className="w-16 rounded border border-border-subtle bg-black/30 px-2 py-1 text-xs text-slate-100"
                        />
                        s
                      </label>
                    </div>
                  </details>
                </div>
              ) : null}
              <div className="rounded-lg border border-border-subtle bg-black/20 p-3">
                <p className="text-xs font-semibold uppercase tracking-wide text-slate-500">{t("auto.ProjectsPanel_tsx.89")}</p>
                <ul className="mt-2 space-y-1 text-xs text-slate-300">
                  {(networkAnalysis?.detection.services ?? []).map((s) => (
                    <li key={`port-${s.name}-${s.port}`}>
                      {s.port} → {s.name} ({t("auto.ProjectsPanel_tsx.90")})
                    </li>
                  ))}
                  {accessMode === "public" ? (
                    <>
                      <li>80 → nginx (public)</li>
                      <li>443 → nginx (https)</li>
                    </>
                  ) : null}
                </ul>
                {accessMode === "public" && !staticNginxFront && !routeRows.length ? (
                  <p className="mt-2 text-xs text-rose-300">{t("auto.ProjectsPanel_tsx.91")}</p>
                ) : null}
              </div>
              <div className="rounded-lg border border-border-subtle bg-black/20 p-3">
                <p className="text-xs font-semibold uppercase tracking-wide text-slate-500">{t("auto.ProjectsPanel_tsx.92")}</p>
                {staticNginxFront ? (
                  <p className="mt-2 text-xs text-slate-400">
                    {tr(
                      "Static nginx-front: vhost из pirate-nginx-snippet.conf (auto-apply при деплое).",
                      "Static nginx-front: vhost from pirate-nginx-snippet.conf (auto-apply on deploy).",
                    )}
                  </p>
                ) : routeRows.length === 0 ? (
                  <p className="mt-2 text-xs text-slate-400">{t("auto.ProjectsPanel_tsx.93")}</p>
                ) : (
                  <div className="mt-2 space-y-1 text-xs text-slate-300">
                    <p className="flex items-center gap-2"><Globe className="h-3.5 w-3.5" /> {t("auto.ProjectsPanel_tsx.94")}: nginx</p>
                    <p className="flex items-center gap-2"><Check className="h-3.5 w-3.5" /> {t("auto.ProjectsPanel_tsx.95")}: {t("auto.ProjectsPanel_tsx.96")}</p>
                    <p>- HTTP → HTTPS {t("auto.ProjectsPanel_tsx.97")}</p>
                    <p>- {t("auto.ProjectsPanel_tsx.98")}</p>
                  </div>
                )}
                <div className="mt-2 flex flex-wrap gap-2">
                  <button
                    type="button"
                    disabled={!endpoint || networkBusy || !deployDir}
                    onClick={() => void onApplyProjectNginx()}
                    className={`${btnBase} border border-red-900/50 bg-red-950/30 text-orange-100`}
                  >
                    {networkBusy ? <Loader2 className="h-4 w-4 animate-spin" /> : null}
                    {language === "ru" ? "Применить nginx проекта на сервер" : "Apply project nginx on server"}
                  </button>
                  <button
                    type="button"
                    disabled={!endpoint}
                    onClick={() => void onSetupDashboardNginx()}
                    className={`${btnBase} border border-border-subtle bg-panel-raised text-slate-200`}
                    title={
                      language === "ru"
                        ? "Общий vhost Pirate: дашборд и /api/ (не UI вашего проекта)"
                        : "Shared Pirate vhost: dashboard and /api/ (not your project UI)"
                    }
                  >
                    {language === "ru" ? "Nginx дашборда Pirate" : "Pirate dashboard nginx"}
                  </button>
                  <button
                    type="button"
                    disabled={!deployDir && !endpoint && !controlApiBase}
                    onClick={() => setNginxConfigModalOpen(true)}
                    className={`${btnBase} border border-border-subtle bg-panel-raised text-slate-200`}
                  >
                    {t("auto.ProjectsPanel_tsx.100")}
                  </button>
                  <button
                    type="button"
                    disabled={!deployDir}
                    onClick={() => setEnvEditorOpen(true)}
                    className={`${btnBase} border border-border-subtle bg-panel-raised text-slate-200`}
                  >
                    {tr("Редактировать env файл", "Edit env file")}
                  </button>
                  <button
                    type="button"
                    disabled={networkBusy || !deployDir}
                    onClick={() => void onRegenerateNetworkConfig()}
                    className={`${btnBase} border border-border-subtle bg-panel-raised text-slate-200`}
                  >
                    {t("auto.ProjectsPanel_tsx.101")}
                  </button>
                </div>
              </div>
              <div className="rounded-lg border border-border-subtle bg-black/20 p-3">
                <p className="text-xs font-semibold uppercase tracking-wide text-slate-500">{t("auto.ProjectsPanel_tsx.102")}</p>
                <div className="mt-2 grid gap-1 text-xs text-slate-300">
                  <label className="inline-flex items-center gap-2">
                    <input
                      type="checkbox"
                      checked={firewallOnlyRequired}
                      onChange={(e) => setFirewallOnlyRequired(e.target.checked)}
                    />
                    {t("auto.ProjectsPanel_tsx.103")}
                  </label>
                  <label className="inline-flex items-center gap-2">
                    <input type="checkbox" checked={ipWhitelist} onChange={(e) => setIpWhitelist(e.target.checked)} />
                    {t("auto.ProjectsPanel_tsx.104")}
                  </label>
                  <label className="inline-flex items-center gap-2">
                    <input type="checkbox" checked={privateRoutes} onChange={(e) => setPrivateRoutes(e.target.checked)} />
                    {t("auto.ProjectsPanel_tsx.105")}
                  </label>
                  <label className="inline-flex items-center gap-2">
                    <input type="checkbox" checked={httpsEnabled} onChange={(e) => setHttpsEnabled(e.target.checked)} />
                    {t("auto.ProjectsPanel_tsx.106")}
                  </label>
                </div>
              </div>
              {networkValidation?.nginxState?.needsUpdate ||
              (networkValidation?.nginxState?.serverTemplateSha256 &&
                networkAnalysis?.localNginxTemplateSha256 &&
                networkValidation.nginxState.serverTemplateSha256 !==
                  networkAnalysis.localNginxTemplateSha256) ? (
                <div className="rounded-lg border border-amber-900/50 bg-amber-950/30 p-3 text-xs text-amber-100">
                  {tr(
                    "Nginx conf изменён — будет обновлён при деплое",
                    "Nginx conf changed — will be updated on deploy",
                  )}
                </div>
              ) : null}
              {networkValidation?.blockers?.length ? (
                <div className="rounded-lg border border-rose-900/50 bg-rose-950/30 p-3 text-xs text-rose-200">
                  <p className="font-semibold">{t("auto.ProjectsPanel_tsx.107")}</p>
                  <ul className="mt-1 space-y-1">
                    {networkValidation.blockers.map((b) => (
                      <li key={b}>- {b}</li>
                    ))}
                  </ul>
                </div>
              ) : null}
              {networkValidation?.warnings?.length ? (
                <div className="rounded-lg border border-orange-900/50 bg-orange-950/30 p-3 text-xs text-orange-100">
                  <p className="font-semibold">{t("auto.ProjectsPanel_tsx.108")}</p>
                  <ul className="mt-1 space-y-1">
                    {networkValidation.warnings.map((w) => (
                      <li key={w}>- {w}</li>
                    ))}
                  </ul>
                </div>
              ) : null}
              <div className="rounded-lg border border-border-subtle bg-black/20 p-3 text-xs">
                <p className="font-semibold uppercase tracking-wide text-slate-500">{t("auto.ProjectsPanel_tsx.109")}</p>
                <div className="mt-2 space-y-1 text-slate-300">
                  <p className="flex items-center gap-2"><ShieldCheck className="h-3.5 w-3.5 text-emerald-300" /> {t("auto.ProjectsPanel_tsx.110")}</p>
                  <p className="flex items-center gap-2"><ShieldCheck className="h-3.5 w-3.5 text-emerald-300" /> {t("auto.ProjectsPanel_tsx.111")}</p>
                  {!httpsEnabled ? (
                    <p className="flex items-center gap-2 text-orange-200">
                      <Lock className="h-3.5 w-3.5" /> {t("auto.ProjectsPanel_tsx.112")}
                    </p>
                  ) : null}
                  {accessMode === "public" && publicHosts().length === 0 ? (
                    <p className="flex items-center gap-2 text-orange-200">
                      <AlertCircle className="h-3.5 w-3.5" /> {t("auto.ProjectsPanel_tsx.113")}
                    </p>
                  ) : null}
                </div>
                <div className="mt-2 flex flex-wrap gap-2">
                  <button
                    type="button"
                    disabled={networkBusy || !deployDir}
                    onClick={() => void refreshNetworkAccess()}
                    className={`${btnBase} border border-border-subtle bg-panel-raised text-slate-200`}
                  >
                    {t("auto.ProjectsPanel_tsx.114")}
                  </button>
                  {!endpoint ? (
                    <p className="flex items-center gap-2 text-xs text-orange-200/90">
                      <AlertCircle className="h-4 w-4 shrink-0" />
                      {t("auto.ProjectsPanel_tsx.115")}
                    </p>
                  ) : null}
                </div>
              </div>
              {networkMsg ? <p className="text-xs text-slate-400">{networkMsg}</p> : null}
            </div>
            {deleteCheck?.uploaded ? (
              <div className="flex flex-wrap gap-2 border-t border-border-subtle pt-4">
                <p className="w-full text-xs text-rose-300/85">
                  {t("auto.ProjectsPanel_tsx.116")} ({deleteCheck.projectId}, {t("auto.ProjectsPanel_tsx.117")} {deleteCheck.currentVersion || "—"}).
                </p>
                <button
                  type="button"
                  disabled={deleteBusy}
                  onClick={() => void onDeleteServerProject()}
                  className={`${btnBase} border border-rose-800/60 bg-rose-950/40 text-rose-100`}
                >
                  {deleteBusy ? (
                    <Loader2 className="h-4 w-4 animate-spin" />
                  ) : (
                    <Trash2 className="h-4 w-4" />
                  )}
                  {deleteConfirm ? t("auto.ProjectsPanel_tsx.118") : t("auto.ProjectsPanel_tsx.119")}
                </button>
                {deleteConfirm ? (
                  <button
                    type="button"
                    disabled={deleteBusy}
                    onClick={() => setDeleteConfirm(false)}
                    className={`${btnBase} border border-border-subtle bg-panel-raised text-slate-300`}
                  >
                    {t("auto.ProjectsPanel_tsx.120")}
                  </button>
                ) : null}
              </div>
            ) : null}
            {deleteMsg ? <p className="text-xs text-slate-400">{deleteMsg}</p> : null}
            <button
              type="button"
              onClick={() => setGuidedStep(3)}
              className={`${btnBase} border border-border-subtle bg-panel-raised text-slate-300`}
            >
              {t("auto.ProjectsPanel_tsx.121")}
            </button>
          </div>
        ) : null}

        <section className="mt-8 rounded-lg border border-border-subtle bg-panel">
          <button
            type="button"
            onClick={() => setAdvancedOpen((v) => !v)}
            className="flex w-full items-center justify-between gap-2 rounded-lg p-4 text-left"
            aria-expanded={advancedOpen}
          >
            <span className="text-sm font-semibold text-slate-200">{t("auto.ProjectsPanel_tsx.122")}</span>
            {advancedOpen ? <ChevronDown className="h-5 w-5 text-slate-400" /> : <ChevronRight className="h-5 w-5 text-slate-400" />}
          </button>
          {advancedOpen ? (
            <div className="space-y-4 border-t border-border-subtle px-4 pb-4 pt-3">
              <div>
                <h3 className="text-xs font-semibold uppercase tracking-wide text-slate-500">{t("auto.ProjectsPanel_tsx.123")}</h3>
                <div className="mt-2 flex flex-wrap gap-2">
                  <button
                    type="button"
                    disabled={paasBusy || !paasPath}
                    onClick={() =>
                      void runPaas("Scan", () =>
                        invoke<string>("paas_scan_project", { path: paasPath!, dryRun: false }),
                      )
                    }
                    className={`${btnBase} border border-border-subtle bg-panel-raised text-slate-200`}
                  >
                    Scan
                  </button>
                  <button
                    type="button"
                    disabled={paasBusy || !paasPath}
                    onClick={() =>
                      void runPaas("Apply gen", async () => {
                        await invoke("paas_apply_gen", { path: paasPath! });
                      })
                    }
                    className={`${btnBase} border border-border-subtle bg-panel-raised text-slate-200`}
                  >
                    Apply gen
                  </button>
                </div>
              </div>
              <div>
                <h3 className="text-xs font-semibold uppercase tracking-wide text-slate-500">{t("auto.ProjectsPanel_tsx.124")}</h3>
                <div className="mt-2 flex flex-wrap gap-2">
                  <button
                    type="button"
                    disabled={deploying}
                    onClick={() => void onPickFolder()}
                    className={`${btnBase} border border-border-subtle bg-panel-raised text-slate-200`}
                  >
                    <FolderOpen className="h-4 w-4" />
                    {t("auto.ProjectsPanel_tsx.125")}
                  </button>
                  <button
                    type="button"
                    disabled={deploying || !deployDir}
                    onClick={() => void onDeploy()}
                    className={btnPrimary}
                  >
                    Deploy
                  </button>
                </div>
              </div>
            </div>
          ) : null}
        </section>
      </div>
    </div>
  );

  const contextColumn = (
    <aside className="flex max-h-[min(100vh,720px)] min-h-0 w-full shrink-0 flex-col border-t border-border-subtle bg-panel lg:max-h-none lg:w-[340px] lg:border-l lg:border-t-0">
      {contextProject}
      {contextPreflight}
      <div className="flex min-h-[120px] flex-1 flex-col overflow-hidden lg:min-h-0">{contextLocalLogs}</div>
      {contextDeploy}
      {contextRollback}
      <div className="mt-auto flex min-h-0 w-full min-w-0 flex-col border-t border-border-subtle">
        <LocalToolchainPanel
          report={toolchainReport}
          loading={toolchainLoading}
          err={toolchainErr}
          onRefresh={onRefreshToolchain}
          defaultExpanded={false}
        />
      </div>
    </aside>
  );

  return (
    <div className="flex min-h-0 flex-1 flex-col lg:flex-row">
      {mainColumn}
      {contextColumn}
      {nginxConfigModalOpen ? (
        <div className="fixed inset-0 z-modalNestedHigh flex items-center justify-center bg-black/70 p-4">
          <div
            className="max-h-[90vh] w-full max-w-3xl overflow-hidden rounded-2xl border border-white/15 bg-[#050204] shadow-2xl"
            role="dialog"
            aria-modal="true"
            aria-labelledby="nginx-config-modal-title"
          >
            <div className="flex items-start justify-between gap-3 border-b border-white/10 px-5 py-4">
              <div className="flex items-center gap-2">
                <FileCode className="h-5 w-5 text-amber-200/80" />
                <h3 id="nginx-config-modal-title" className="text-sm font-semibold text-slate-100">
                  {t("auto.ProjectsPanel_tsx.100")}
                </h3>
              </div>
              <button
                type="button"
                className="rounded-lg border border-white/10 bg-white/5 p-1.5 text-slate-400 hover:text-slate-200"
                onClick={() => setNginxConfigModalOpen(false)}
                aria-label={tr("Закрыть", "Close")}
              >
                <X className="h-4 w-4" />
              </button>
            </div>
            <div className="max-h-[min(70vh,560px)] space-y-5 overflow-auto px-5 py-4">
              <section className="rounded-xl border border-white/10 bg-black/25 p-3">
                <h4 className="text-xs font-semibold uppercase tracking-wide text-slate-400">
                  {tr("Локальное превью", "Local preview")}
                </h4>
                <p className="mt-1 text-[11px] leading-relaxed text-slate-500">
                  {tr(
                    "Сгенерировано из pirate.toml и детекта сервисов; домен и маршруты из мастера сети (публичный режим и таблица маршрутов) подмешиваются в превью до сохранения в файл. Отдельно от глобального vhost на хосте и от релизного сниппета в «Проекты на сервере».",
                    "Generated from pirate.toml and detected services; the network wizard domain and route table are merged into this preview before you save the file. Separate from the host vhost and the release snippet under Server projects.",
                  )}
                </p>
                {!deployDir ? (
                  <p className="mt-2 text-sm text-slate-500">{tr("Выберите каталог проекта.", "Pick a project folder.")}</p>
                ) : networkBusy && !networkAnalysis?.nginxPreview ? (
                  <div className="mt-3 flex justify-center py-8 text-slate-500">
                    <Loader2 className="h-7 w-7 animate-spin opacity-70" />
                  </div>
                ) : networkAnalysis?.nginxPreview ? (
                  <pre className="mt-3 whitespace-pre-wrap break-words rounded-lg bg-black/40 p-3 font-mono text-[11px] leading-relaxed text-slate-200">
                    {networkAnalysis.nginxPreview}
                  </pre>
                ) : (
                  <p className="mt-2 text-sm text-slate-400">
                    {tr(
                      "Превью пустое: нажмите «Обновить анализ» в мастере сети или задайте маршруты в pirate.toml и сервисы.",
                      "No preview yet: use “Refresh analysis” in the network wizard or set routes in pirate.toml and services.",
                    )}
                  </p>
                )}
              </section>

              <section className="rounded-xl border border-white/10 bg-black/25 p-3">
                <h4 className="text-xs font-semibold uppercase tracking-wide text-slate-400">
                  {tr("Nginx на сервере (control-api)", "Nginx on server (control-api)")}
                </h4>
                <p className="mt-1 text-[11px] leading-relaxed text-slate-500">
                  {tr(
                    "Файл /etc/nginx/sites-available/pirate — дашборд Pirate. Vhost проекта: /etc/nginx/sites-available/pirate-project-<id>. При [proxy].nginx_auto_apply vhost применяется автоматически после деплоя; иначе — кнопка «Применить nginx проекта». Шаблон: releases/…/pirate-nginx-snippet.conf (<PATH_PROJECT>, <VERSION>).",
                    "File /etc/nginx/sites-available/pirate is the Pirate dashboard. Project vhost: /etc/nginx/sites-available/pirate-project-<id>. With [proxy].nginx_auto_apply the vhost is applied automatically after deploy; otherwise use “Apply project nginx on server”. Template: releases/…/pirate-nginx-snippet.conf (<PATH_PROJECT>, <VERSION>).",
                  )}
                </p>
                {!controlApiBase?.trim() ? (
                  <p className="mt-2 text-sm text-orange-200/90">
                    {tr(
                      "HTTP base для control-api не задан: подключитесь к deploy-server или укажите URL в настройках закладки. Нужен JWT — войдите в «Проекты на сервере».",
                      "control-api HTTP base is not set: connect to deploy-server or set the URL in bookmark settings. JWT required — sign in under Server projects.",
                    )}
                  </p>
                ) : serverNginxLoading ? (
                  <div className="mt-3 flex justify-center py-8 text-slate-500">
                    <Loader2 className="h-7 w-7 animate-spin opacity-70" />
                  </div>
                ) : serverNginxErr ? (
                  <p className="mt-2 whitespace-pre-wrap text-sm text-rose-300/90">{serverNginxErr}</p>
                ) : (
                  <>
                    {serverNginxPath ? (
                      <p className="mt-3 font-mono text-[11px] text-slate-500">{serverNginxPath}</p>
                    ) : null}
                    <pre className="mt-2 whitespace-pre-wrap break-words rounded-lg bg-black/40 p-3 font-mono text-[11px] leading-relaxed text-slate-200">
                      {serverNginxContent ?? ""}
                    </pre>
                  </>
                )}
              </section>
            </div>
          </div>
        </div>
      ) : null}
      {envEditorOpen ? (
        <div className="fixed inset-0 z-modalNestedHigh flex items-center justify-center bg-black/70 p-4">
          <div
            className="flex max-h-[90vh] w-full max-w-3xl flex-col overflow-hidden rounded-2xl border border-white/15 bg-[#050204] shadow-2xl"
            role="dialog"
            aria-modal="true"
            aria-labelledby="env-editor-modal-title"
          >
            <div className="flex items-start justify-between gap-3 border-b border-white/10 px-5 py-4">
              <div className="flex items-center gap-2">
                <FileCode className="h-5 w-5 text-amber-200/80" />
                <h3 id="env-editor-modal-title" className="text-sm font-semibold text-slate-100">
                  {tr("Env файл: ", "Env file: ")}
                  <span className="font-mono text-amber-200/90">{envFilePath}</span>
                </h3>
              </div>
              <button
                type="button"
                className="rounded-lg border border-white/10 bg-white/5 p-1.5 text-slate-400 hover:text-slate-200"
                onClick={() => setEnvEditorOpen(false)}
                aria-label={tr("Закрыть", "Close")}
              >
                <X className="h-4 w-4" />
              </button>
            </div>
            <div className="flex-1 overflow-auto px-5 py-4">
              <textarea
                className="h-96 w-full rounded-lg border border-white/10 bg-black/40 p-3 font-mono text-xs leading-relaxed text-slate-200 focus:outline-none focus:ring-1 focus:ring-amber-400/40"
                value={envFileContent}
                onChange={(e) => setEnvFileContent(e.target.value)}
                spellCheck={false}
              />
              {envError && <p className="mt-2 text-sm text-rose-300/90">{envError}</p>}
            </div>
            <div className="flex justify-end gap-2 border-t border-white/10 px-5 py-3">
              <button
                type="button"
                className={`${btnBase} border border-white/10 bg-white/5 text-slate-300 hover:text-slate-100`}
                onClick={() => setEnvEditorOpen(false)}
              >
                {tr("Закрыть", "Close")}
              </button>
              <button
                type="button"
                disabled={envSaving}
                className={`${btnBase} border border-amber-500/30 bg-amber-500/15 text-amber-200 hover:bg-amber-500/25 disabled:opacity-50`}
                onClick={() => void saveEnvFile()}
              >
                {envSaving ? tr("Сохраняем...", "Saving...") : tr("Сохранить", "Save")}
              </button>
            </div>
          </div>
        </div>
      ) : null}
      {cmdVarsModal}
    </div>
  );
}
