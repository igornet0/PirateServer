/**
 * Настройки удалённого сервера через control-api (JWT): статус, окружение (host env + app.env), перезапуск процесса.
 * Требует входа в control-api; base URL задаётся для этого сохранённого gRPC URL.
 */
import { invoke } from "@tauri-apps/api/core";
import { AlertCircle, Loader2, Settings, X } from "lucide-react";
import React, { useCallback, useEffect, useMemo, useRef, useState } from "react";
import { suggestControlApiFromGrpcUrl } from "./controlApiUrl";
import { HostServerEnvPanel } from "./serverDeployEnv/HostServerEnvPanel";
import { parseDotEnv } from "./serverDeployEnv/parseSerialize";
import { HostServicesPanel } from "./HostServicesPanel";
import { useI18n } from "./i18n";
import { CopyablePre } from "./ui/CopyablePre";
import { ModalDialog } from "./ui/ModalDialog";
import { useControlApiSession } from "./session/ControlApiSession";

const AntiDdosPanel = React.lazy(() =>
  import("./AntiDdosPanel").then((m) => ({ default: m.AntiDdosPanel })),
);
const HostTerminalPanel = React.lazy(() =>
  import("./HostTerminalPanel").then((m) => ({ default: m.HostTerminalPanel })),
);
const ProcessListenersPanel = React.lazy(() =>
  import("./ProcessListenersPanel").then((m) => ({ default: m.ProcessListenersPanel })),
);
const SslManagementPanel = React.lazy(() =>
  import("./SslManagementPanel").then((m) => ({ default: m.SslManagementPanel })),
);

const tabPanelFallback = (
  <div className="flex items-center gap-2 py-8 text-sm text-slate-400">
    <Loader2 className="h-4 w-4 animate-spin" />
    …
  </div>
);

const btnBase =
  "inline-flex items-center justify-center gap-2 rounded-xl px-4 py-2.5 text-sm font-semibold transition-all duration-200 focus-visible:outline-none focus-visible:ring-2 focus-visible:ring-red-600/80 focus-visible:ring-offset-2 focus-visible:ring-offset-[#050204] active:scale-[0.98] disabled:pointer-events-none disabled:opacity-50";

type ControlApiKeychainCreds = { username: string; password: string };

type NginxSiteRow = {
  site_id: string;
  file_name: string;
  path: string;
  entry_kind: string;
  active: boolean;
  enabled: boolean;
  enabled_path?: string | null;
  domains: string[];
  ssl_enabled: boolean;
  listen_ports: number[];
  managed_by: string;
  is_ui_stack: boolean;
  parse_warnings: string[];
};

type NginxMiniDialog =
  | { kind: "domain"; path: string; draft: string }
  | { kind: "ssl"; path: string; enable: boolean };

type NginxSitesPayload = {
  ok: boolean;
  nginx_test_output?: string | null;
  global_warnings: string[];
  global_conflicts: { level: string; code: string; message: string }[];
  sites: NginxSiteRow[];
};

type NginxPreflightPayload = {
  ok: boolean;
  inventory: NginxSitesPayload;
  blockers: { level: string; code: string; message: string }[];
};

function normalizeNginxSiteRow(raw: unknown, index: number): NginxSiteRow {
  const row = raw as Partial<NginxSiteRow> | null;
  return {
    site_id: typeof row?.site_id === "string" && row.site_id.length > 0 ? row.site_id : `row-${index}`,
    file_name: typeof row?.file_name === "string" ? row.file_name : "unknown.conf",
    path: typeof row?.path === "string" ? row.path : "",
    entry_kind: typeof row?.entry_kind === "string" ? row.entry_kind : "vhost",
    active: Boolean(row?.active),
    enabled: Boolean(row?.enabled),
    enabled_path: typeof row?.enabled_path === "string" ? row.enabled_path : null,
    domains: Array.isArray(row?.domains) ? row.domains.filter((v): v is string => typeof v === "string") : [],
    ssl_enabled: Boolean(row?.ssl_enabled),
    listen_ports: Array.isArray(row?.listen_ports)
      ? row.listen_ports.filter((v): v is number => typeof v === "number")
      : [],
    managed_by: typeof row?.managed_by === "string" ? row.managed_by : "unknown",
    is_ui_stack: Boolean(row?.is_ui_stack),
    parse_warnings: Array.isArray(row?.parse_warnings)
      ? row.parse_warnings.filter((v): v is string => typeof v === "string")
      : [],
  };
}

function normalizeNginxSitesPayload(raw: unknown): NginxSitesPayload {
  const payload = raw as Partial<NginxSitesPayload> | null;
  return {
    ok: Boolean(payload?.ok),
    nginx_test_output:
      typeof payload?.nginx_test_output === "string" || payload?.nginx_test_output === null
        ? payload.nginx_test_output
        : null,
    global_warnings: Array.isArray(payload?.global_warnings)
      ? payload.global_warnings.filter((v): v is string => typeof v === "string")
      : [],
    global_conflicts: Array.isArray(payload?.global_conflicts)
      ? payload.global_conflicts.filter(
          (v): v is { level: string; code: string; message: string } =>
            typeof v === "object" &&
            v !== null &&
            typeof (v as { message?: unknown }).message === "string" &&
            typeof (v as { level?: unknown }).level === "string" &&
            typeof (v as { code?: unknown }).code === "string",
        )
      : [],
    sites: Array.isArray(payload?.sites) ? payload.sites.map((row, index) => normalizeNginxSiteRow(row, index)) : [],
  };
}

function normalizeNginxPreflightPayload(raw: unknown): NginxPreflightPayload {
  const payload = raw as Partial<NginxPreflightPayload> | null;
  return {
    ok: Boolean(payload?.ok),
    inventory: normalizeNginxSitesPayload(payload?.inventory),
    blockers: Array.isArray(payload?.blockers)
      ? payload.blockers.filter(
          (v): v is { level: string; code: string; message: string } =>
            typeof v === "object" &&
            v !== null &&
            typeof (v as { message?: unknown }).message === "string" &&
            typeof (v as { level?: unknown }).level === "string" &&
            typeof (v as { code?: unknown }).code === "string",
        )
      : [],
  };
}

export type ServerBookmark = {
  id: string;
  label: string;
  url: string;
  /** Out-of-band host-agent (optional). */
  host_agent_base_url?: string | null;
  host_agent_token?: string | null;
};

function normalizeGrpcUrl(s: string): string {
  return s.trim().replace(/\/+$/, "");
}

function waitForNextFrame(): Promise<void> {
  return new Promise((resolve) => requestAnimationFrame(() => resolve()));
}

function sleep(ms: number): Promise<void> {
  return new Promise((resolve) => window.setTimeout(resolve, ms));
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

type TabId =
  | "connect"
  | "terminal"
  | "info"
  | "env"
  | "services"
  | "antiddos"
  | "nginx"
  | "ssl"
  | "process";

type Props = {
  open: boolean;
  onClose: () => void;
  bookmark: ServerBookmark;
  /** Активное gRPC подключение (если есть). */
  activeEndpoint: string | null;
  /** Текущий сохранённый HTTP base control-api в приложении. */
  savedControlApiBase: string;
  /** После успешного переименования закладки (обновить список в родителе). */
  onBookmarkRenamed?: () => void | Promise<void>;
  /** Поддерживает ли текущий server-stack UI (по server-stack-manifest), null если неизвестно. */
  hostUiBundled?: boolean | null;
};

/**
 * One nginx-inventory site row. Memoized so editing the nginx file `<textarea>`
 * (or any of this modal's ~59 state hooks) does not re-render every site row —
 * only the row whose `row`/`highlighted`/`busy` props actually change. All
 * callbacks passed in are stable (`useCallback` / state setters).
 */
const NginxSiteTableRow = React.memo(function NginxSiteTableRow({
  row,
  highlighted,
  busy,
  onOpenFile,
  onAction,
  onMiniDialog,
}: {
  row: NginxSiteRow;
  highlighted: boolean;
  busy: boolean;
  onOpenFile: (path: string) => void;
  onAction: (body: Record<string, unknown>) => void;
  onMiniDialog: (dialog: NginxMiniDialog) => void;
}) {
  const { language } = useI18n();
  const tr = (ru: string, en: string) => (language === "ru" ? ru : en);
  const vhost = row.entry_kind === "vhost";
  const activeLabel = vhost
    ? row.active
      ? tr("да (sites-enabled)", "yes (enabled)")
      : tr("нет", "no")
    : tr("вкл. в main", "via main");
  return (
    <tr
      className={`border-b border-white/5 hover:bg-white/[0.03] ${
        highlighted ? "bg-cyan-950/20 ring-1 ring-inset ring-cyan-700/30" : ""
      }`}
    >
      <td className="px-2 py-1.5 align-top">
        <button
          type="button"
          disabled={busy}
          onClick={() => onOpenFile(row.path)}
          title={tr("Открыть содержимое файла", "Open file contents")}
          className="w-full max-w-[240px] rounded-lg border border-transparent px-1 py-0.5 text-left transition-colors hover:border-white/15 hover:bg-white/[0.05] focus-visible:outline-none focus-visible:ring-2 focus-visible:ring-amber-600/50 disabled:opacity-50"
        >
          <div className="font-mono text-[10px] text-amber-100/80 break-all">
            {row.file_name}
          </div>
          <div className="mt-0.5 break-all text-[9px] text-slate-500">{row.path}</div>
        </button>
      </td>
      <td className="px-2 py-1.5 align-top text-slate-400">{row.entry_kind}</td>
      <td className="px-2 py-1.5 align-top text-slate-300">{activeLabel}</td>
      <td className="px-2 py-1.5 align-top">
        <span
          className={
            row.managed_by === "pirate"
              ? "rounded border border-cyan-700/50 bg-cyan-950/40 px-1.5 py-0.5 text-cyan-200"
              : "text-slate-500"
          }
        >
          {row.managed_by}
        </span>
      </td>
      <td className="px-2 py-1.5 align-top">
        {row.ssl_enabled ? (
          <span className="text-emerald-300">on</span>
        ) : (
          <span className="text-slate-500">off</span>
        )}
      </td>
      <td className="px-2 py-1.5 align-top">
        {row.is_ui_stack ? <span className="text-violet-300">UI</span> : "—"}
      </td>
      <td className="px-2 py-1.5 align-top break-all text-slate-400">
        {row.domains.join(", ")}
      </td>
      <td className="px-2 py-1.5 align-top">
        <div className="flex max-w-[280px] flex-wrap gap-1">
          {vhost && !row.active ? (
            <button
              type="button"
              disabled={busy}
              onClick={() =>
                onAction({ action: "enable_site", available_path: row.path })
              }
              className="rounded border border-white/10 bg-white/5 px-1.5 py-0.5 text-[10px] hover:bg-white/10"
            >
              {tr("Вкл.", "Enable")}
            </button>
          ) : null}
          {vhost && row.active ? (
            <button
              type="button"
              disabled={busy}
              onClick={() => {
                const ep =
                  row.enabled_path && row.enabled_path.length > 0
                    ? row.enabled_path
                    : `/etc/nginx/sites-enabled/${row.file_name}`;
                onAction({ action: "disable_site", enabled_path: ep });
              }}
              className="rounded border border-rose-800/30 bg-rose-950/30 px-1.5 py-0.5 text-[10px] text-rose-100"
            >
              {tr("Выкл.", "Disable")}
            </button>
          ) : null}
          <button
            type="button"
            disabled={busy}
            onClick={() => {
              const initial =
                row.domains[0] && row.domains[0] !== "_" ? row.domains[0] : "";
              onMiniDialog({ kind: "domain", path: row.path, draft: initial });
            }}
            className="rounded border border-white/10 bg-white/5 px-1.5 py-0.5 text-[10px] hover:bg-white/10"
          >
            {tr("Домен", "Domain")}
          </button>
          <button
            type="button"
            disabled={busy}
            onClick={() => {
              onMiniDialog({
                kind: "ssl",
                path: row.path,
                enable: !row.ssl_enabled,
              });
            }}
            className="rounded border border-white/10 bg-white/5 px-1.5 py-0.5 text-[10px] hover:bg-white/10"
          >
            SSL {row.ssl_enabled ? tr("off", "off") : tr("on", "on")}
          </button>
        </div>
      </td>
    </tr>
  );
});

export function ServerBookmarkSettingsModal({
  open,
  onClose,
  bookmark,
  activeEndpoint,
  savedControlApiBase,
  onBookmarkRenamed,
  hostUiBundled = null,
}: Props) {
  const { language, t } = useI18n();
  const { ensureControlApiBase } = useControlApiSession();
  const tr = (ru: string, en: string) => (language === "ru" ? ru : en);
    const [tab, setTab] = useState<TabId>("connect");
  const [listLabelDraft, setListLabelDraft] = useState("");
  const [listLabelBusy, setListLabelBusy] = useState(false);
  const [listLabelErr, setListLabelErr] = useState<string | null>(null);
  const [controlBase, setControlBase] = useState("");
  const [user, setUser] = useState("");
  const [pass, setPass] = useState("");
  const [rememberInKeychain, setRememberInKeychain] = useState(false);
  const [keychainBanner, setKeychainBanner] = useState<string | null>(null);
  const [loginBusy, setLoginBusy] = useState(false);
  const [sessionOk, setSessionOk] = useState(false);
  const [err, setErr] = useState<string | null>(null);
  const [restartPendingUntil, setRestartPendingUntil] = useState(0);

  const [projectId, setProjectId] = useState("default");
  const [projectsLoading, setProjectsLoading] = useState(false);

  const [statusJson, setStatusJson] = useState<string | null>(null);
  const [statusBusy, setStatusBusy] = useState(false);

  /** Сервер: `/etc/pirate-deploy.env`; приложение: `app.env` в каталоге project. */
  const [envSection, setEnvSection] = useState<"host" | "app">("host");
  const [hostEnvText, setHostEnvText] = useState("");
  const [hostEnvPath, setHostEnvPath] = useState<string | null>(null);
  const [hostEnvExists, setHostEnvExists] = useState(false);
  const [hostEnvBusy, setHostEnvBusy] = useState(false);
  const [hostEnvDirty, setHostEnvDirty] = useState(false);
  const [hostRestartHint, setHostRestartHint] = useState<string | null>(null);
  const hostEnvGrpcPublicSnapshotRef = useRef<string>("");
  const [grpcPublicUrlSaveClientHint, setGrpcPublicUrlSaveClientHint] = useState<string | null>(null);

  const [envText, setEnvText] = useState("");
  const [envPath, setEnvPath] = useState<string | null>(null);
  const [envExists, setEnvExists] = useState(false);
  const [envBusy, setEnvBusy] = useState(false);
  const [envDirty, setEnvDirty] = useState(false);
  const [nginxStatus, setNginxStatus] = useState<{
    installed?: boolean;
    version?: string | null;
    systemd_active?: string | null;
    site_config_path?: string;
    site_file_exists?: boolean;
    site_enabled?: boolean;
    ensure_script_present?: boolean;
    apply_site_script_present?: boolean;
    ops_script_present?: boolean;
  } | null>(null);
  const [nginxSiteText, setNginxSiteText] = useState("");
  const [nginxSitePath, setNginxSitePath] = useState<string | null>(null);
  const [nginxSiteBusy, setNginxSiteBusy] = useState(false);
  const [nginxSiteDirty, setNginxSiteDirty] = useState(false);
  const [nginxEnsureBusy, setNginxEnsureBusy] = useState(false);
  const [nginxOut, setNginxOut] = useState<string | null>(null);
  const [nginxEnvUpdate, setNginxEnvUpdate] = useState<{
    mode?: string;
    restart_scheduled?: boolean;
    updates?: { key?: string; old_value?: string | null; new_value?: string | null }[];
  } | null>(null);
  const [confirmRemoveNginx, setConfirmRemoveNginx] = useState(false);
  const [nginxProgressOpen, setNginxProgressOpen] = useState(false);
  const [nginxProgressTitle, setNginxProgressTitle] = useState("Nginx operation");
  const [nginxProgressValue, setNginxProgressValue] = useState(0);
  const [nginxCancelRequested, setNginxCancelRequested] = useState(false);
  const nginxProgressTimer = useRef<number | null>(null);
  const nginxOpSeq = useRef(0);
  const [nginxSitesPayload, setNginxSitesPayload] = useState<NginxSitesPayload | null>(null);
  const [nginxPreflightBlockers, setNginxPreflightBlockers] = useState<
    { level: string; code: string; message: string }[]
  >([]);
  /** Tauri WebView often blocks `window.prompt` / `window.confirm`; use inline UI instead. */
  const [nginxMiniDialog, setNginxMiniDialog] = useState<NginxMiniDialog | null>(
    null,
  );
  const [nginxFileEditor, setNginxFileEditor] = useState<{
    path: string;
    content: string;
    dirty: boolean;
    loading: boolean;
    readOnly: boolean;
    readOnlyReason: string | null;
  } | null>(null);

  const [restartBusy, setRestartBusy] = useState(false);
  const [stopBusy, setStopBusy] = useState(false);
  const [restartOut, setRestartOut] = useState<string | null>(null);
  const [processStatus, setProcessStatus] = useState<{
    current_version?: string;
    state?: string;
    source?: string;
  } | null>(null);
  const [processStatusBusy, setProcessStatusBusy] = useState(false);

  const [haBase, setHaBase] = useState("");
  const [haToken, setHaToken] = useState("");
  const [haOut, setHaOut] = useState<string | null>(null);
  const [haBusy, setHaBusy] = useState(false);
  const [haSaveBusy, setHaSaveBusy] = useState(false);
  const [haStackVersion, setHaStackVersion] = useState("");
  const restartPending = restartPendingUntil > Date.now();

  const sameServerAsActive = useMemo(() => {
    if (!activeEndpoint) return false;
    return normalizeGrpcUrl(bookmark.url) === normalizeGrpcUrl(activeEndpoint);
  }, [bookmark.url, activeEndpoint]);

  const prefillBase = useCallback(() => {
    if (sameServerAsActive && savedControlApiBase.trim()) {
      return savedControlApiBase.trim();
    }
    return suggestControlApiFromGrpcUrl(bookmark.url) ?? "";
  }, [sameServerAsActive, savedControlApiBase, bookmark.url]);

  useEffect(() => {
    if (!open) return;
    setListLabelDraft(bookmark.label);
    setListLabelErr(null);
    setTab("connect");
    setErr(null);
    const base = prefillBase();
    setControlBase(base);
    setStatusJson(null);
    setRestartOut(null);
    setEnvDirty(false);
    setHostEnvDirty(false);
    setHostRestartHint(null);
    setGrpcPublicUrlSaveClientHint(null);
    setEnvSection("host");
    setNginxStatus(null);
    setNginxSiteText("");
    setNginxSitePath(null);
    setNginxSiteDirty(false);
    setNginxOut(null);
    setNginxEnvUpdate(null);
    setConfirmRemoveNginx(false);
    setNginxProgressOpen(false);
    setNginxProgressValue(0);
    setNginxCancelRequested(false);
    setNginxSitesPayload(null);
    setNginxPreflightBlockers([]);
    setHaBase((bookmark.host_agent_base_url ?? "").trim());
    setHaToken(bookmark.host_agent_token ?? "");
    setHaOut(null);
    setHaStackVersion("");
    setKeychainBanner(null);
    void (async () => {
      try {
        if (sameServerAsActive) {
          const p = await invoke<string>("get_active_project");
          setProjectId(p?.trim() || "default");
        } else {
          setProjectId("default");
        }
      } catch {
        setProjectId("default");
      }
      try {
        const cur = await invoke<string | null>("get_control_api_base");
        const ok = await invoke<boolean>("control_api_session_active");
        setSessionOk(
          Boolean(ok && cur && normalizeGrpcUrl(cur) === normalizeGrpcUrl(base)),
        );
      } catch {
        setSessionOk(false);
      }
    })();
  }, [open, prefillBase, sameServerAsActive, bookmark.label, bookmark.host_agent_base_url, bookmark.host_agent_token]);

  const saveListLabel = async () => {
    const label = listLabelDraft.trim();
    if (!label) {
      setListLabelErr(t("auto.ServerBookmarkSettingsModal_tsx.1"));
      return;
    }
    if (label === bookmark.label) {
      setListLabelErr(null);
      return;
    }
    setListLabelBusy(true);
    setListLabelErr(null);
    try {
      await invoke("rename_server_bookmark", { id: bookmark.id, label });
      await onBookmarkRenamed?.();
    } catch (e) {
      setListLabelErr(String(e));
    } finally {
      setListLabelBusy(false);
    }
  };

  const saveHostAgentFields = async () => {
    setHaSaveBusy(true);
    setErr(null);
    try {
      await invoke("save_bookmark_host_agent", {
        id: bookmark.id,
        host_agent_base_url: haBase.trim(),
        host_agent_token: haToken,
      });
      await onBookmarkRenamed?.();
    } catch (e) {
      setErr(String(e));
    } finally {
      setHaSaveBusy(false);
    }
  };

  const hostAgentPingHealth = async () => {
    if (!haBase.trim()) {
      setErr(tr("Укажите base URL host-agent.", "Set host-agent base URL."));
      return;
    }
    setHaBusy(true);
    setHaOut(null);
    setErr(null);
    try {
      const j = await invoke<string>("host_agent_health_json", { base_url: haBase.trim() });
      setHaOut(j);
    } catch (e) {
      setErr(String(e));
    } finally {
      setHaBusy(false);
    }
  };

  const hostAgentFetchStatus = async () => {
    if (!haBase.trim() || !haToken.trim()) {
      setErr(tr("Нужны base URL и токен.", "Base URL and token required."));
      return;
    }
    setHaBusy(true);
    setHaOut(null);
    setErr(null);
    try {
      const j = await invoke<string>("host_agent_status_json", {
        base_url: haBase.trim(),
        token: haToken.trim(),
      });
      setHaOut(j);
    } catch (e) {
      setErr(String(e));
    } finally {
      setHaBusy(false);
    }
  };

  const hostAgentReboot = async () => {
    if (!haBase.trim() || !haToken.trim()) {
      setErr(tr("Нужны base URL и токен.", "Base URL and token required."));
      return;
    }
    if (
      !window.confirm(
        tr(
          "Запланировать перезагрузку хоста через host-agent?",
          "Schedule host reboot via host-agent?",
        ),
      )
    ) {
      return;
    }
    setHaBusy(true);
    setHaOut(null);
    setErr(null);
    try {
      const j = await invoke<string>("host_agent_reboot_json", {
        base_url: haBase.trim(),
        token: haToken.trim(),
        delay_sec: 60,
      });
      setHaOut(j);
    } catch (e) {
      setErr(String(e));
    } finally {
      setHaBusy(false);
    }
  };

  const hostAgentUploadStack = async () => {
    if (!haBase.trim() || !haToken.trim()) {
      setErr(tr("Нужны base URL и токен.", "Base URL and token required."));
      return;
    }
    const ver = haStackVersion.trim();
    if (!ver) {
      setErr(tr("Укажите версию бандла (как в OTA).", "Enter bundle version label (as for OTA)."));
      return;
    }
    setHaBusy(true);
    setHaOut(null);
    setErr(null);
    try {
      const path = await invoke<string | null>("pick_server_stack_tar_gz");
      if (!path) {
        setHaBusy(false);
        return;
      }
      const j = await invoke<string>("host_agent_upload_server_stack_cmd", {
        base_url: haBase.trim(),
        token: haToken.trim(),
        path,
        version: ver,
      });
      setHaOut(j);
    } catch (e) {
      setErr(String(e));
    } finally {
      setHaBusy(false);
    }
  };

  const loadProjectsHint = useCallback(async () => {
    setProjectsLoading(true);
    setErr(null);
    try {
      await ensureControlApiBase(controlBase.trim());
      const overview = await invoke<{ projects: { id: string }[] }>("fetch_server_projects_overview");
      const ids = overview.projects.map((p) => p.id);
      if (ids.length && !ids.includes(projectId)) {
        setProjectId(ids[0]!);
      }
    } catch (e) {
      setErr(String(e));
    } finally {
      setProjectsLoading(false);
    }
  }, [controlBase, projectId]);

  const tryFillFromKeychain = useCallback(async (base: string) => {
    if (!base.trim()) return;
    try {
      const c = await invoke<ControlApiKeychainCreds | null>("control_api_keychain_load", {
        baseUrl: base.trim(),
      });
      if (c?.username != null && c.username !== "") {
        setUser(c.username);
        setPass(c.password ?? "");
      }
    } catch {
      /* Tauri unavailable or keychain — ignore */
    }
  }, []);

  useEffect(() => {
    if (!open || sessionOk) return;
    const b = controlBase.trim();
    if (!b) return;
    void tryFillFromKeychain(b);
  }, [open, sessionOk, controlBase, tryFillFromKeychain]);

  const onForgetControlApiKeychain = useCallback(async () => {
    const base = controlBase.trim();
    if (!base) return;
    setErr(null);
    setKeychainBanner(null);
    try {
      await invoke("control_api_keychain_delete", { baseUrl: base });
      setUser("");
      setPass("");
      setKeychainBanner(
        tr(
          "Учётные данные удалены из связки ключей (Keychain / системное хранилище).",
          "Credentials removed from the password store (Keychain / OS vault).",
        ),
      );
    } catch (e) {
      setErr(String(e));
    }
  }, [controlBase, tr]);

  const onLogin = async () => {
    setLoginBusy(true);
    setErr(null);
    setKeychainBanner(null);
    try {
      await ensureControlApiBase(controlBase.trim());
      const waitUntilReady = async () => {
        if (!restartPending) return;
        const base = controlBase.trim();
        for (let i = 0; i < 8; i += 1) {
          try {
            const probe = await invoke<string>("control_api_health_probe", { baseUrl: base });
            if (probe.includes("health_http=200")) return;
          } catch {
            // keep waiting inside restart window
          }
          await sleep(600);
        }
      };
      await waitUntilReady();
      let lastErr: unknown = null;
      for (let i = 0; i < 3; i += 1) {
        try {
          await invoke("control_api_login", {
            baseUrl: controlBase.trim(),
            username: user.trim(),
            password: pass,
          });
          lastErr = null;
          break;
        } catch (e) {
          lastErr = e;
          if (!isNetworkTimeoutLike(String(e)) || i === 2) break;
          await sleep(450 * (i + 1));
        }
      }
      if (lastErr) throw lastErr;
      const baseTrim = controlBase.trim();
      let keychainWarn: string | null = null;
      if (rememberInKeychain && baseTrim) {
        try {
          await invoke("control_api_keychain_save", {
            baseUrl: baseTrim,
            username: user.trim(),
            password: pass,
          });
        } catch (kcErr) {
          keychainWarn = tr(
            `Вход выполнен, но не удалось сохранить в связке ключей: ${String(kcErr)}`,
            `Signed in, but saving to the password store failed: ${String(kcErr)}`,
          );
        }
      }
      setPass("");
      setUser("");
      setSessionOk(true);
      setKeychainBanner(keychainWarn);
      await loadProjectsHint();
    } catch (e) {
      setSessionOk(false);
      setErr(String(e));
    } finally {
      setLoginBusy(false);
    }
  };

  const onLogout = async () => {
    setErr(null);
    try {
      await invoke("control_api_logout");
      setSessionOk(false);
    } catch (e) {
      setErr(String(e));
    }
  };

  const loadStatus = async () => {
    setStatusBusy(true);
    setErr(null);
    try {
      await ensureControlApiBase(controlBase.trim());
      const raw = await invoke<string>("control_api_fetch_status_json", { projectId });
      setStatusJson(raw);
    } catch (e) {
      setStatusJson(null);
      setErr(String(e));
    } finally {
      setStatusBusy(false);
    }
  };

  const loadAppEnv = useCallback(async () => {
    setEnvBusy(true);
    setErr(null);
    try {
      await ensureControlApiBase(controlBase.trim());
      const raw = await invoke<string>("control_api_fetch_app_env_json", { projectId });
      const parsed = JSON.parse(raw) as { path?: string; content?: string; exists?: boolean };
      setEnvPath(typeof parsed.path === "string" ? parsed.path : null);
      setEnvExists(Boolean(parsed.exists));
      setEnvText(typeof parsed.content === "string" ? parsed.content : "");
      setEnvDirty(false);
    } catch (e) {
      setEnvPath(null);
      setEnvText("");
      setErr(String(e));
    } finally {
      setEnvBusy(false);
    }
  }, [controlBase, projectId]);

  const loadHostEnv = useCallback(async () => {
    setHostEnvBusy(true);
    setErr(null);
    setHostRestartHint(null);
    try {
      await ensureControlApiBase(controlBase.trim());
      const raw = await invoke<string>("control_api_fetch_host_deploy_env_json");
      const parsed = JSON.parse(raw) as { path?: string; content?: string; exists?: boolean };
      setHostEnvPath(typeof parsed.path === "string" ? parsed.path : null);
      setHostEnvExists(Boolean(parsed.exists));
      setHostEnvText(typeof parsed.content === "string" ? parsed.content : "");
      setHostEnvDirty(false);
      const hostContent = typeof parsed.content === "string" ? parsed.content : "";
      hostEnvGrpcPublicSnapshotRef.current =
        parseDotEnv(hostContent).get("DEPLOY_GRPC_PUBLIC_URL")?.trim() ?? "";
    } catch (e) {
      setHostEnvPath(null);
      setHostEnvText("");
      setErr(String(e));
    } finally {
      setHostEnvBusy(false);
    }
  }, [controlBase]);

  const saveAppEnv = async () => {
    setEnvBusy(true);
    setErr(null);
    try {
      await ensureControlApiBase(controlBase.trim());
      await invoke("control_api_put_app_env", { projectId, content: envText });
      setEnvDirty(false);
      await loadAppEnv();
    } catch (e) {
      setErr(String(e));
    } finally {
      setEnvBusy(false);
    }
  };

  const saveHostEnv = async () => {
    setHostEnvBusy(true);
    setErr(null);
    setHostRestartHint(null);
    try {
      await ensureControlApiBase(controlBase.trim());
      const raw = await invoke<string>("control_api_put_host_deploy_env", {
        content: hostEnvText,
      });
      let scheduled = false;
      try {
        const j = JSON.parse(raw) as { restart_scheduled?: boolean };
        scheduled = Boolean(j.restart_scheduled);
      } catch {
        scheduled = raw.includes("restart_scheduled") && raw.includes("true");
      }
      setHostRestartHint(
        scheduled
          ? tr(
              "Запланирован перезапуск deploy-server и control-api (через несколько секунд). Сессия JWT может прерваться — при необходимости войдите снова.",
              "Restart of deploy-server and control-api is scheduled (in a few seconds). JWT session may break; sign in again if needed.",
            )
          : tr(
              "Файл записан. При отсутствии systemd или helper-скрипта перезапустите сервисы вручную.",
              "File saved. If systemd/helper script is unavailable, restart services manually.",
            ),
      );
      if (scheduled) {
        setRestartPendingUntil(Date.now() + 90_000);
        await invoke("mark_control_api_recent_restart", { seconds: 90 });
      }
      const mapNow = parseDotEnv(hostEnvText);
      const nowGrpc = mapNow.get("DEPLOY_GRPC_PUBLIC_URL")?.trim() ?? "";
      const wasGrpc = hostEnvGrpcPublicSnapshotRef.current;
      if (nowGrpc && nowGrpc !== wasGrpc) {
        setGrpcPublicUrlSaveClientHint(
          tr(
            "На этом ПК обновите сохранённый gRPC endpoint: вкладка «Обзор» или «Соединение» → «Перейти на объявленный URL» (после перезапуска сервера, если адрес изменился).",
            "On this PC, update the saved gRPC endpoint: Overview or Connection → «Switch to advertised URL» (after the server restarts if the address changed).",
          ),
        );
      }
      setHostEnvDirty(false);
      await loadHostEnv();
    } catch (e) {
      setErr(String(e));
    } finally {
      setHostEnvBusy(false);
    }
  };

  const applyHostEnvTemplate = async () => {
    setHostEnvBusy(true);
    setErr(null);
    try {
      await ensureControlApiBase(controlBase.trim());
      const raw = await invoke<string>("control_api_fetch_host_deploy_env_template_json");
      const parsed = JSON.parse(raw) as { template?: string };
      if (typeof parsed.template === "string" && parsed.template.length > 0) {
        setHostEnvText(parsed.template);
        setHostEnvDirty(true);
      }
    } catch (e) {
      setErr(String(e));
    } finally {
      setHostEnvBusy(false);
    }
  };

  useEffect(() => {
    if (!open || tab !== "env" || !sessionOk) return;
    if (envSection === "host") void loadHostEnv();
    else void loadAppEnv();
  }, [open, tab, sessionOk, envSection, loadHostEnv, loadAppEnv]);

  const loadProcessStatus = useCallback(async () => {
    if (!sessionOk) return;
    setProcessStatusBusy(true);
    try {
      await ensureControlApiBase(controlBase.trim());
      const raw = await invoke<string>("control_api_fetch_status_json", { projectId });
      const parsed = JSON.parse(raw) as {
        current_version?: string;
        state?: string;
        source?: string;
      };
      setProcessStatus(parsed);
    } catch {
      setProcessStatus(null);
    } finally {
      setProcessStatusBusy(false);
    }
  }, [controlBase, projectId, sessionOk]);

  const restartProcess = async () => {
    setRestartBusy(true);
    setErr(null);
    setRestartOut(null);
    try {
      await ensureControlApiBase(controlBase.trim());
      const raw = await invoke<string>("control_api_restart_process_json", { projectId });
      setRestartOut(raw);
      await loadProcessStatus();
    } catch (e) {
      setErr(String(e));
    } finally {
      setRestartBusy(false);
    }
  };

  const stopProcess = async () => {
    setStopBusy(true);
    setErr(null);
    setRestartOut(null);
    try {
      await ensureControlApiBase(controlBase.trim());
      const raw = await invoke<string>("control_api_stop_process_json", { projectId });
      setRestartOut(raw);
      await loadProcessStatus();
    } catch (e) {
      setErr(String(e));
    } finally {
      setStopBusy(false);
    }
  };

  const loadNginxStatus = useCallback(async () => {
    setNginxSiteBusy(true);
    setErr(null);
    try {
      await ensureControlApiBase(controlBase.trim());
      const raw = await invoke<string>("control_api_fetch_nginx_status_json");
      const parsed = JSON.parse(raw) as typeof nginxStatus;
      setNginxStatus(parsed ?? null);
    } catch (e) {
      setNginxStatus(null);
      setErr(String(e));
    } finally {
      setNginxSiteBusy(false);
    }
  }, [controlBase]);

  const loadNginxSite = useCallback(async () => {
    setNginxSiteBusy(true);
    setErr(null);
    try {
      await ensureControlApiBase(controlBase.trim());
      const raw = await invoke<string>("control_api_fetch_nginx_site_json");
      const parsed = JSON.parse(raw) as { path?: string; content?: string };
      setNginxSitePath(typeof parsed.path === "string" ? parsed.path : null);
      setNginxSiteText(typeof parsed.content === "string" ? parsed.content : "");
      setNginxSiteDirty(false);
    } catch (e) {
      setNginxSitePath(null);
      setNginxSiteText("");
      setErr(String(e));
    } finally {
      setNginxSiteBusy(false);
    }
  }, [controlBase]);

  const loadNginxInventory = useCallback(async () => {
    setNginxSiteBusy(true);
    setErr(null);
    try {
      await ensureControlApiBase(controlBase.trim());
      const raw = await invoke<string>("control_api_fetch_nginx_sites_json");
      const parsed = normalizeNginxSitesPayload(JSON.parse(raw));
      setNginxSitesPayload(parsed);
    } catch (e) {
      setNginxSitesPayload(null);
      setErr(String(e));
    } finally {
      setNginxSiteBusy(false);
    }
  }, [controlBase]);

  const runNginxPreflight = useCallback(async () => {
    setNginxSiteBusy(true);
    setErr(null);
    try {
      await ensureControlApiBase(controlBase.trim());
      const raw = await invoke<string>("control_api_nginx_preflight_json", { body: "{}" });
      const parsed = normalizeNginxPreflightPayload(JSON.parse(raw));
      setNginxSitesPayload(parsed.inventory);
      setNginxPreflightBlockers(parsed.blockers ?? []);
    } catch (e) {
      setErr(String(e));
    } finally {
      setNginxSiteBusy(false);
    }
  }, [controlBase]);

  const runNginxAction = useCallback(
    async (body: Record<string, unknown>) => {
      setNginxSiteBusy(true);
      setErr(null);
      setNginxOut(null);
      try {
        await ensureControlApiBase(controlBase.trim());
        const raw = await invoke<string>("control_api_nginx_action_json", { body: JSON.stringify(body) });
        setNginxOut(raw);
        try {
          const j = JSON.parse(raw) as {
            ok?: boolean;
            message?: string;
            detail?: string | null;
            post_check?: {
              classified?: string;
              summary?: string;
              probe_host?: string | null;
              curl_exit?: number | null;
            };
          };
          if (typeof j.ok === "boolean" && j.ok === false) {
            const detail =
              j.detail != null && String(j.detail).trim() !== "" ? `\n${String(j.detail).trim()}` : "";
            let msg = `${j.message ?? tr("Операция nginx не удалась", "Nginx action failed")}${detail}`;
            if (j.post_check?.classified === "tls_name_mismatch") {
              msg += `\n${tr(
                "Сертификат не соответствует имени хоста (SNI). Проверьте server_name в vhost и CN/SAN сертификата, либо задайте post_check_host.",
                "Certificate does not match the hostname (SNI). Check server_name in the vhost and the certificate CN/SAN, or set post_check_host explicitly.",
              )}`;
            }
            setErr(msg);
          }
        } catch {
          /* non-JSON body */
        }
        await loadNginxInventory();
        setNginxPreflightBlockers([]);
        await loadNginxStatus();
        await loadNginxSite();
      } catch (e) {
        setErr(String(e));
      } finally {
        setNginxSiteBusy(false);
      }
    },
    [controlBase, loadNginxInventory, loadNginxStatus, loadNginxSite, tr, nginxSitesPayload],
  );

  const saveNginxSite = async () => {
    setNginxSiteBusy(true);
    setErr(null);
    setNginxOut(null);
    try {
      await ensureControlApiBase(controlBase.trim());
      const raw = await invoke<string>("control_api_put_nginx_site", { content: nginxSiteText });
      setNginxOut(raw);
      setNginxSiteDirty(false);
      await loadNginxStatus();
      await loadNginxSite();
    } catch (e) {
      setErr(String(e));
    } finally {
      setNginxSiteBusy(false);
    }
  };

  const ensureNginx = async (mode: "api_only" | "with_ui" | "remove") => {
    const opId = ++nginxOpSeq.current;
    setNginxEnsureBusy(true);
    setErr(null);
    setNginxOut(null);
    setNginxEnvUpdate(null);
    setConfirmRemoveNginx(false);
    setNginxProgressTitle(
      mode === "remove" ? t("auto.ServerBookmarkSettingsModal_tsx.2") : t("auto.ServerBookmarkSettingsModal_tsx.3"),
    );
    setNginxProgressOpen(true);
    setNginxProgressValue(8);
    setNginxCancelRequested(false);
    if (nginxProgressTimer.current !== null) {
      window.clearInterval(nginxProgressTimer.current);
    }
    nginxProgressTimer.current = window.setInterval(() => {
      setNginxProgressValue((v) => {
        if (v >= 92) return v;
        const step = v < 40 ? 7 : v < 70 ? 4 : 2;
        return Math.min(92, v + step);
      });
    }, 350);
    try {
      // Let React paint the overlay before invoking blocking host operation.
      await waitForNextFrame();
      await waitForNextFrame();
      await ensureControlApiBase(controlBase.trim());
      const raw = await invoke<string>("control_api_ensure_nginx", { mode });
      if (opId !== nginxOpSeq.current || nginxCancelRequested) {
        return;
      }
      setNginxOut(raw);
      try {
        const parsed = JSON.parse(raw) as {
          env_update?: {
            mode?: string;
            restart_scheduled?: boolean;
            updates?: { key?: string; old_value?: string | null; new_value?: string | null }[];
          };
        };
        setNginxEnvUpdate(parsed.env_update ?? null);
        if (parsed.env_update?.restart_scheduled) {
          setRestartPendingUntil(Date.now() + 90_000);
          await invoke("mark_control_api_recent_restart", { seconds: 90 });
        }
      } catch {
        setNginxEnvUpdate(null);
      }
      setNginxProgressValue(100);
      await loadNginxStatus();
      await loadNginxSite();
      await loadNginxInventory();
      setNginxPreflightBlockers([]);
      await loadHostEnv();
    } catch (e) {
      if (opId !== nginxOpSeq.current || nginxCancelRequested) {
        return;
      }
      setErr(String(e));
    } finally {
      if (nginxProgressTimer.current !== null) {
        window.clearInterval(nginxProgressTimer.current);
        nginxProgressTimer.current = null;
      }
      if (!nginxCancelRequested) {
        window.setTimeout(() => {
          if (opId === nginxOpSeq.current) setNginxProgressOpen(false);
        }, 350);
      }
      setNginxEnsureBusy(false);
    }
  };

  useEffect(() => {
    return () => {
      if (nginxProgressTimer.current !== null) {
        window.clearInterval(nginxProgressTimer.current);
      }
    };
  }, []);

  useEffect(() => {
    if (!open || tab !== "nginx" || !sessionOk) return;
    void loadNginxStatus();
    void loadNginxSite();
    void loadNginxInventory();
  }, [open, tab, sessionOk, loadNginxStatus, loadNginxSite, loadNginxInventory]);

  useEffect(() => {
    if (!open || tab !== "process" || !sessionOk) return;
    void loadProcessStatus();
  }, [open, tab, sessionOk, loadProcessStatus]);

  const allowApiWithUiMode = hostUiBundled !== false;
  const hiddenHostEnvKeys = useMemo(() => {
    if (allowApiWithUiMode) return undefined;
    return new Set<string>([
      "CONTROL_UI_ADMIN_USERNAME",
      "CONTROL_UI_ADMIN_PASSWORD",
      "CONTROL_API_JWT_SECRET",
      "CONTROL_UI_ADMIN_PASSWORD_RESET",
      "DEPLOY_DASHBOARD_PASSWORD",
    ]);
  }, [allowApiWithUiMode]);

  const tabs: { id: TabId; label: string }[] = [
    { id: "connect", label: t("auto.ServerBookmarkSettingsModal_tsx.4") },
    { id: "terminal", label: tr("Терминал", "Terminal") },
    { id: "info", label: t("auto.ServerBookmarkSettingsModal_tsx.5") },
    { id: "env", label: t("auto.ServerBookmarkSettingsModal_tsx.6") },
    { id: "services", label: tr("Сервисы", "Services") },
    { id: "antiddos", label: tr("Anti-DDoS", "Anti-DDoS") },
    { id: "nginx", label: "nginx" },
    { id: "ssl", label: tr("SSL", "SSL") },
    { id: "process", label: t("auto.ServerBookmarkSettingsModal_tsx.7") },
  ];
  const nginxInstalled = Boolean(nginxStatus?.installed);

  const goTab = (id: TabId) => {
    setErr(null);
    if (id !== "nginx") {
      setNginxMiniDialog(null);
      setNginxFileEditor(null);
    }
    setTab(id);
  };

  const openNginxFileEditor = useCallback(
    (filePath: string) => {
      const readOnly = filePath.includes("/sites-enabled/");
      setNginxFileEditor({
        path: filePath,
        content: "",
        dirty: false,
        loading: true,
        readOnly,
        readOnlyReason: readOnly
          ? tr(
              "Это запись в sites-enabled (симлинк) — только просмотр. Редактируйте файл в sites-available.",
              "This is a sites-enabled symlink — read-only. Edit the file under sites-available.",
            )
          : null,
      });
      void (async () => {
        try {
          setErr(null);
          await ensureControlApiBase(controlBase.trim());
          const raw = await invoke<string>("control_api_fetch_nginx_file_json", { path: filePath });
          const j = JSON.parse(raw) as { content?: string };
          setNginxFileEditor((prev) =>
            prev && prev.path === filePath
              ? {
                  ...prev,
                  content: typeof j.content === "string" ? j.content : "",
                  loading: false,
                  dirty: false,
                }
              : prev,
          );
        } catch (e) {
          setErr(String(e));
          setNginxFileEditor(null);
        }
      })();
    },
    [controlBase, tr],
  );

  const reloadNginxFileEditor = useCallback(() => {
    const p = nginxFileEditor?.path;
    if (!p) return;
    setNginxFileEditor((prev) => (prev ? { ...prev, loading: true } : null));
    void (async () => {
      try {
        setErr(null);
        await ensureControlApiBase(controlBase.trim());
        const raw = await invoke<string>("control_api_fetch_nginx_file_json", { path: p });
        const j = JSON.parse(raw) as { content?: string };
        setNginxFileEditor((prev) =>
          prev && prev.path === p
            ? {
                ...prev,
                content: typeof j.content === "string" ? j.content : "",
                loading: false,
                dirty: false,
              }
            : prev,
        );
      } catch (e) {
        setErr(String(e));
        setNginxFileEditor((prev) => (prev ? { ...prev, loading: false } : null));
      }
    })();
  }, [nginxFileEditor?.path, controlBase]);

  const saveNginxInventoryFile = useCallback(async () => {
    if (!nginxFileEditor || nginxFileEditor.readOnly || !nginxFileEditor.dirty) return;
    setNginxSiteBusy(true);
    setErr(null);
    setNginxOut(null);
    try {
      await ensureControlApiBase(controlBase.trim());
      const raw = await invoke<string>("control_api_put_nginx_file_json", {
        path: nginxFileEditor.path,
        content: nginxFileEditor.content,
      });
      setNginxOut(raw);
      try {
        const j = JSON.parse(raw) as { ok?: boolean; message?: string; detail?: string | null };
        if (typeof j.ok === "boolean" && j.ok === false) {
          const detail =
            j.detail != null && String(j.detail).trim() !== "" ? `\n${String(j.detail).trim()}` : "";
          setErr(
            `${j.message ?? tr("Сохранение nginx не удалось", "Saving nginx config failed")}${detail}`,
          );
        } else {
          setNginxFileEditor((prev) => (prev ? { ...prev, dirty: false } : null));
        }
      } catch {
        setNginxFileEditor((prev) => (prev ? { ...prev, dirty: false } : null));
      }
      await loadNginxInventory();
      await loadNginxStatus();
      await loadNginxSite();
    } catch (e) {
      setErr(String(e));
    } finally {
      setNginxSiteBusy(false);
    }
  }, [
    nginxFileEditor,
    controlBase,
    loadNginxInventory,
    loadNginxStatus,
    loadNginxSite,
    tr,
  ]);

  return (
    <>
      <ModalDialog
        open={open}
        onClose={onClose}
        zClassName="z-modalServerSettings"
        closeOnBackdrop={false}
        closeOnEscape={!nginxProgressOpen}
        panelClassName={
          tab === "terminal"
            ? "w-full max-w-7xl max-h-[90vh] min-h-0"
            : tab === "ssl"
              ? "w-full max-w-5xl max-h-[90vh] min-h-0"
              : "w-full max-w-4xl max-h-[90vh] min-h-0"
        }
        aria-labelledby="srv-settings-title"
      >
        <div className="max-h-[100vh] w-full overflow-hidden rounded-2xl border border-white/10 bg-[#0a0908] shadow-2xl shadow-black/60">
        <div className="flex items-start justify-between gap-3 border-b border-white/10 px-5 py-4">
          <div className="min-w-0">
            <h2 id="srv-settings-title" className="flex items-center gap-2 text-lg font-semibold text-slate-100">
              <Settings className="h-5 w-5 shrink-0 text-red-400" aria-hidden />
              <span className="truncate">{t("auto.ServerBookmarkSettingsModal_tsx.8")}: {bookmark.label}</span>
            </h2>
            <p className="mt-1 break-all font-mono text-xs text-amber-200/75">{bookmark.url}</p>
            {sameServerAsActive ? (
              <p className="mt-1 text-xs text-slate-500">{t("auto.ServerBookmarkSettingsModal_tsx.9")}</p>
            ) : (
              <p className="mt-1 text-xs text-orange-200/85">
                {t("auto.ServerBookmarkSettingsModal_tsx.10")}
              </p>
            )}
          </div>
          <button
            type="button"
            onClick={onClose}
            className={`${btnBase} shrink-0 border border-white/10 bg-white/5 p-2 text-slate-300 hover:bg-white/10`}
            aria-label={t("auto.ServerBookmarkSettingsModal_tsx.11")}
          >
            <X className="h-4 w-4" />
          </button>
        </div>

        <div className="-mx-0 flex flex-nowrap gap-1 overflow-x-auto border-b border-white/10 px-2 pt-2">
          {tabs.map((tabItem) => (
            <button
              key={tabItem.id}
              type="button"
              onClick={() => goTab(tabItem.id)}
              disabled={tabItem.id !== "connect" && !sessionOk}
              className={`shrink-0 rounded-t-lg px-3 py-2 text-sm font-medium transition-colors duration-150 ${
                tab === tabItem.id
                  ? "bg-white/10 text-slate-100"
                  : "text-slate-500 hover:bg-white/5 hover:text-slate-300"
              } disabled:cursor-not-allowed disabled:opacity-40`}
            >
              {tabItem.label}
            </button>
          ))}
        </div>

        <div className="max-h-[calc(90vh-11rem)] overflow-y-auto px-5 py-4">
          {err ? (
            <p className="mb-3 flex items-start gap-2 text-sm text-rose-300">
              <AlertCircle className="mt-0.5 h-4 w-4 shrink-0" />
              {err}
            </p>
          ) : null}

          {tab === "connect" ? (
            <div className="space-y-4">
              <div className="rounded-xl border border-white/10 bg-black/25 p-4">
                <p className="text-sm font-semibold text-slate-200">{t("auto.ServerBookmarkSettingsModal_tsx.12")}</p>
                <p className="mt-1 text-xs text-slate-500">
                  Подпись в «Saved servers» (только текст; URL gRPC не меняется).
                </p>
                <label className="mt-3 block text-xs font-medium text-slate-500" htmlFor="bookmark-list-label">
                  Label
                </label>
                <input
                  id="bookmark-list-label"
                  value={listLabelDraft}
                  onChange={(e) => {
                    setListLabelDraft(e.target.value);
                    setListLabelErr(null);
                  }}
                  className="mt-1 w-full rounded-lg border border-white/10 bg-black/30 px-3 py-2 text-sm text-slate-100 placeholder:text-slate-600 focus:border-red-600/50 focus:outline-none"
                  placeholder="Production"
                  autoComplete="off"
                />
                {listLabelErr ? (
                  <p className="mt-2 text-sm text-rose-300">{listLabelErr}</p>
                ) : null}
                <div className="mt-3 flex flex-wrap gap-2">
                  <button
                    type="button"
                    disabled={
                      listLabelBusy ||
                      listLabelDraft.trim() === bookmark.label ||
                      !listLabelDraft.trim()
                    }
                    onClick={() => void saveListLabel()}
                    className={`${btnBase} border border-red-800/45 bg-red-950/40 text-orange-100 hover:bg-red-950/55 disabled:opacity-40`}
                  >
                    {listLabelBusy ? <Loader2 className="h-4 w-4 animate-spin" /> : null}
                    {t("auto.ServerBookmarkSettingsModal_tsx.13")}
                  </button>
                </div>
              </div>

              <div>
                <label className="mb-1 block text-xs font-medium uppercase tracking-wide text-slate-500">
                  Control API (HTTP)
                </label>
                <input
                  type="url"
                  value={controlBase}
                  onChange={(e) => setControlBase(e.target.value)}
                  placeholder="http://192.168.x.x"
                  className="w-full rounded-lg border border-white/10 bg-black/30 px-3 py-2 font-mono text-sm text-slate-100 placeholder:text-slate-600 focus:border-red-700/45 focus:outline-none"
                />
                <p className="mt-1 text-xs text-slate-500">
                  {tr(
                    "Обычно это тот же хост, что и gRPC, без порта (nginx на :80/:443). Если reverse proxy нет — укажите явно ",
                    "Usually it is the same host as gRPC, without port (nginx on :80/:443). If there is no reverse proxy, set ",
                  )}
                  <code className="text-slate-400">http://IP:8080</code> {t("auto.ServerBookmarkSettingsModal_tsx.14")}{" "}
                  <code className="text-slate-400">CONTROL_API_BIND=0.0.0.0</code>.{" "}
                  {tr(
                    "После входа JWT сохраняется для этого base URL (смена URL в других окнах сбрасывает сессию).",
                    "After login JWT is saved for this base URL (changing URL in other windows resets the session).",
                  )}
                </p>
              </div>
              <div className="grid gap-3 sm:grid-cols-2">
                <div>
                  <label className="mb-1 block text-xs text-slate-500">{t("auto.ServerBookmarkSettingsModal_tsx.15")}</label>
                  <input
                    value={user}
                    onChange={(e) => setUser(e.target.value)}
                    onFocus={() => {
                      const b = controlBase.trim();
                      if (!b) return;
                      if (!user.trim() && !pass.trim()) {
                        void tryFillFromKeychain(b);
                      }
                    }}
                    autoComplete="username"
                    className="w-full rounded-lg border border-white/10 bg-black/30 px-3 py-2 text-sm text-slate-100 focus:border-red-600/50 focus:outline-none"
                  />
                </div>
                <div>
                  <label className="mb-1 block text-xs text-slate-500">{t("auto.ServerBookmarkSettingsModal_tsx.16")}</label>
                  <input
                    type="password"
                    value={pass}
                    onChange={(e) => setPass(e.target.value)}
                    autoComplete="current-password"
                    className="w-full rounded-lg border border-white/10 bg-black/30 px-3 py-2 text-sm text-slate-100 focus:border-red-600/50 focus:outline-none"
                  />
                </div>
              </div>
              <div className="flex flex-wrap gap-2">
                <button
                  type="button"
                  disabled={loginBusy || !controlBase.trim()}
                  onClick={() => void onLogin()}
                  className={`${btnBase} bg-gradient-to-r from-red-700 to-red-900 text-white shadow-lg shadow-red-950/40 hover:brightness-110 disabled:opacity-40`}
                >
                  {loginBusy ? <Loader2 className="h-4 w-4 animate-spin" /> : null}
                  {t("auto.ServerBookmarkSettingsModal_tsx.17")}
                </button>
                <button
                  type="button"
                  onClick={() => void onLogout()}
                  className={`${btnBase} border border-white/15 bg-white/5 text-slate-200 hover:bg-white/10`}
                >
                  {t("auto.ServerBookmarkSettingsModal_tsx.18")}
                </button>
                <span
                  className={`inline-flex items-center rounded-full px-3 py-1 text-xs font-medium ${
                    sessionOk
                      ? "bg-emerald-500/15 text-emerald-300 ring-1 ring-emerald-500/35"
                      : "bg-slate-600/20 text-slate-400"
                  }`}
                >
                  {sessionOk ? t("auto.ServerBookmarkSettingsModal_tsx.19") : t("auto.ServerBookmarkSettingsModal_tsx.20")}
                </span>
              </div>
              <div className="flex flex-wrap items-center justify-between gap-2 border-t border-white/10 pt-3">
                <label className="inline-flex cursor-pointer items-center gap-2 text-xs text-slate-400">
                  <input
                    type="checkbox"
                    checked={rememberInKeychain}
                    onChange={(e) => setRememberInKeychain(e.target.checked)}
                    className="rounded border-white/20 bg-black/40 text-red-600 focus:ring-red-600/60"
                  />
                  <span
                    title={tr(
                      "После успешного входа сохранить логин и пароль в связке ключей macOS (или в системном хранилище на других ОС).",
                      "After a successful sign-in, save username and password in macOS Keychain (or the OS credential store elsewhere).",
                    )}
                  >
                    {tr("Сохранить в связке ключей", "Save in password store")}
                  </span>
                </label>
                <button
                  type="button"
                  className="text-xs text-slate-500 underline decoration-slate-600 underline-offset-2 hover:text-slate-300"
                  onClick={() => void onForgetControlApiKeychain()}
                >
                  {tr("Удалить из связки ключей", "Remove from password store")}
                </button>
              </div>
              {keychainBanner ? <p className="text-xs text-slate-400">{keychainBanner}</p> : null}

              <div className="rounded-xl border border-amber-900/35 bg-amber-950/20 p-4">
                <p className="text-sm font-semibold text-amber-100/95">
                  {tr("Host-agent (вне control-api / gRPC)", "Host-agent (bypasses control-api / gRPC)")}
                </p>
                <p className="mt-1 text-xs text-slate-500">
                  {tr(
                    "Отдельный HTTP-сервис на хосте: OTA бандла и перезагрузка, если deploy-server или control-api недоступны. Токен в /etc/pirate-host-agent.env на сервере.",
                    "Separate on-host HTTP service: stack tarball and reboot when deploy-server or control-api are down. Token is in /etc/pirate-host-agent.env on the server.",
                  )}
                </p>
                <label className="mt-3 block text-xs text-slate-500" htmlFor="ha-base">
                  {tr("Base URL", "Base URL")}
                </label>
                <input
                  id="ha-base"
                  value={haBase}
                  onChange={(e) => setHaBase(e.target.value)}
                  placeholder="http://127.0.0.1:9443"
                  className="mt-1 w-full rounded-lg border border-white/10 bg-black/30 px-3 py-2 font-mono text-sm text-slate-100 placeholder:text-slate-600 focus:border-amber-700/45 focus:outline-none"
                  autoComplete="off"
                />
                <label className="mt-2 block text-xs text-slate-500" htmlFor="ha-token">
                  Bearer token
                </label>
                <input
                  id="ha-token"
                  type="password"
                  value={haToken}
                  onChange={(e) => setHaToken(e.target.value)}
                  placeholder="••••••••"
                  className="mt-1 w-full rounded-lg border border-white/10 bg-black/30 px-3 py-2 font-mono text-sm text-slate-100 focus:border-amber-700/45 focus:outline-none"
                  autoComplete="off"
                />
                <label className="mt-2 block text-xs text-slate-500" htmlFor="ha-ver">
                  {tr("Версия для OTA-архива (server-stack)", "Bundle version label (server-stack)")}
                </label>
                <input
                  id="ha-ver"
                  value={haStackVersion}
                  onChange={(e) => setHaStackVersion(e.target.value)}
                  placeholder="1.2.3"
                  className="mt-1 w-full rounded-lg border border-white/10 bg-black/30 px-3 py-2 font-mono text-sm text-slate-100 focus:outline-none"
                  autoComplete="off"
                />
                <div className="mt-3 flex flex-wrap gap-2">
                  <button
                    type="button"
                    disabled={haSaveBusy}
                    onClick={() => void saveHostAgentFields()}
                    className={`${btnBase} border border-amber-800/40 bg-amber-950/40 text-amber-50 hover:bg-amber-950/55`}
                  >
                    {haSaveBusy ? <Loader2 className="h-4 w-4 animate-spin" /> : null}
                    {tr("Сохранить в закладке", "Save on bookmark")}
                  </button>
                  <button
                    type="button"
                    disabled={haBusy || !haBase.trim()}
                    onClick={() => void hostAgentPingHealth()}
                    className={`${btnBase} border border-white/15 bg-white/5 text-slate-200 hover:bg-white/10`}
                  >
                    GET /health
                  </button>
                  <button
                    type="button"
                    disabled={haBusy || !haBase.trim() || !haToken.trim()}
                    onClick={() => void hostAgentFetchStatus()}
                    className={`${btnBase} border border-white/15 bg-white/5 text-slate-200 hover:bg-white/10`}
                  >
                    /v1/status
                  </button>
                  <button
                    type="button"
                    disabled={haBusy || !haBase.trim() || !haToken.trim()}
                    onClick={() => void hostAgentReboot()}
                    className={`${btnBase} border border-rose-900/50 bg-rose-950/35 text-rose-100 hover:bg-rose-950/50`}
                  >
                    {tr("Перезагрузка хоста", "Reboot host")}
                  </button>
                  <button
                    type="button"
                    disabled={haBusy || !haBase.trim() || !haToken.trim() || !haStackVersion.trim()}
                    onClick={() => void hostAgentUploadStack()}
                    className={`${btnBase} border border-amber-800/40 bg-amber-900/30 text-amber-50 hover:bg-amber-900/45`}
                  >
                    {tr("Загрузить бандл (OTA)", "Upload bundle (OTA)")}
                  </button>
                </div>
                {haOut ? (
                  <pre className="mt-3 max-h-40 overflow-auto rounded-lg border border-white/10 bg-black/40 p-2 text-xs text-slate-300">
                    {haOut}
                  </pre>
                ) : null}
              </div>
            </div>
          ) : null}

          {tab === "terminal" && sessionOk ? (
            <React.Suspense fallback={tabPanelFallback}>
              <HostTerminalPanel controlBase={controlBase} tr={tr} restartPending={restartPending} />
            </React.Suspense>
          ) : null}

          {tab === "info" && sessionOk ? (
            <div className="space-y-3">
              <div className="flex flex-wrap items-end gap-2">
                <div className="min-w-[8rem] flex-1">
                  <label className="mb-1 block text-xs text-slate-500">{t("auto.ServerBookmarkSettingsModal_tsx.21")}</label>
                  <input
                    value={projectId}
                    onChange={(e) => setProjectId(e.target.value)}
                    className="w-full rounded-lg border border-white/10 bg-black/30 px-3 py-2 font-mono text-sm text-slate-100 focus:outline-none"
                  />
                </div>
                <button
                  type="button"
                  disabled={projectsLoading}
                  onClick={() => void loadProjectsHint()}
                  className={`${btnBase} border border-white/15 bg-white/5 text-slate-200 hover:bg-white/10`}
                >
                  {projectsLoading ? <Loader2 className="h-4 w-4 animate-spin" /> : null}
                  {t("auto.ServerBookmarkSettingsModal_tsx.22")}
                </button>
                <button
                  type="button"
                  disabled={statusBusy}
                  onClick={() => void loadStatus()}
                  className={`${btnBase} border border-red-800/40 bg-amber-950/30 text-amber-100 hover:bg-amber-950/50`}
                >
                  {statusBusy ? <Loader2 className="h-4 w-4 animate-spin" /> : null}
                  {t("auto.ServerBookmarkSettingsModal_tsx.23")}
                </button>
              </div>
              <p className="text-xs text-slate-500">
                {t("auto.ServerBookmarkSettingsModal_tsx.24")} <code className="text-orange-200/85">GET /api/v1/status</code>{" "}
                {t("auto.ServerBookmarkSettingsModal_tsx.25")}
              </p>
              <CopyablePre
                value={statusJson}
                placeholder={t("auto.ServerBookmarkSettingsModal_tsx.26")}
                className="rounded-xl border border-white/10 bg-black/40 p-3 text-xs text-emerald-100/90"
                maxHeightClass="max-h-64"
              />
            </div>
          ) : null}

          {tab === "env" && sessionOk ? (
            <div className="space-y-3">
              <div className="flex flex-wrap gap-2">
              <button
                type="button"
                onClick={() => {
                  setErr(null);
                  setEnvSection("host");
                }}
                  className={`rounded-lg px-3 py-1.5 text-xs font-semibold ${
                    envSection === "host"
                      ? "bg-amber-900/50 text-amber-100 ring-1 ring-amber-600/50"
                      : "bg-white/5 text-slate-400 hover:bg-white/10"
                  }`}
                >
                  {t("auto.ServerBookmarkSettingsModal_tsx.27")}
                </button>
                <button
                  type="button"
                  onClick={() => {
                    setErr(null);
                    setEnvSection("app");
                  }}
                  className={`rounded-lg px-3 py-1.5 text-xs font-semibold ${
                    envSection === "app"
                      ? "bg-amber-900/50 text-amber-100 ring-1 ring-amber-600/50"
                      : "bg-white/5 text-slate-400 hover:bg-white/10"
                  }`}
                >
                  {t("auto.ServerBookmarkSettingsModal_tsx.28")}
                </button>
              </div>

              {envSection === "host" ? (
                <>
                  <p className="text-xs text-slate-400">
                    {t("auto.ServerBookmarkSettingsModal_tsx.29")}:{" "}
                    <code className="break-all text-amber-200/85">{hostEnvPath ?? "—"}</code>
                    {hostEnvExists ? (
                      <span className="ml-2 text-emerald-400/90">{t("auto.ServerBookmarkSettingsModal_tsx.30")}</span>
                    ) : (
                      <span className="ml-2 text-slate-500">
                        {t("auto.ServerBookmarkSettingsModal_tsx.31")}
                      </span>
                    )}
                  </p>
                  <p className="text-xs text-slate-500">
                    {t("auto.ServerBookmarkSettingsModal_tsx.32")} <code className="text-slate-400">env.example</code>{" "}
                    {t("auto.ServerBookmarkSettingsModal_tsx.33")}:{" "}
                    <code className="text-slate-400">DEPLOY_*</code>, <code className="text-slate-400">CONTROL_API_*</code>{" "}
                    {t("auto.ServerBookmarkSettingsModal_tsx.34")}{" "}
                    <code className="text-slate-400">deploy-server</code> {t("auto.ServerBookmarkSettingsModal_tsx.35")}{" "}
                    <code className="text-slate-400">control-api</code> {t("auto.ServerBookmarkSettingsModal_tsx.36")}
                  </p>
                  {hostRestartHint ? (
                    <p className="rounded-lg border border-amber-700/40 bg-amber-950/30 px-3 py-2 text-xs text-amber-100/95">
                      {hostRestartHint}
                    </p>
                  ) : null}
                  {grpcPublicUrlSaveClientHint ? (
                    <p className="rounded-lg border border-sky-700/40 bg-sky-950/30 px-3 py-2 text-xs text-sky-100/95">
                      {grpcPublicUrlSaveClientHint}
                    </p>
                  ) : null}
                  <HostServerEnvPanel
                    value={hostEnvText}
                    disabled={hostEnvBusy}
                    hiddenKeys={hiddenHostEnvKeys}
                    onChange={(s) => {
                      setHostEnvText(s);
                      setHostEnvDirty(true);
                      setHostRestartHint(null);
                    }}
                  />
                  <div className="flex flex-wrap gap-2">
                    <button
                      type="button"
                      disabled={hostEnvBusy}
                      onClick={() => void loadHostEnv()}
                      className={`${btnBase} border border-white/15 bg-white/5 text-slate-200 hover:bg-white/10`}
                    >
                      {hostEnvBusy ? <Loader2 className="h-4 w-4 animate-spin" /> : null}
                      {t("auto.ServerBookmarkSettingsModal_tsx.37")}
                    </button>
                    <button
                      type="button"
                      disabled={hostEnvBusy}
                      onClick={() => void applyHostEnvTemplate()}
                      className={`${btnBase} border border-white/15 bg-white/5 text-slate-200 hover:bg-white/10`}
                    >
                      {t("auto.ServerBookmarkSettingsModal_tsx.38")}
                    </button>
                    <button
                      type="button"
                      disabled={hostEnvBusy || !hostEnvDirty}
                      onClick={() => void saveHostEnv()}
                      className={`${btnBase} bg-gradient-to-r from-red-700 to-red-900 text-white shadow-lg shadow-red-950/40 hover:brightness-110 disabled:opacity-40`}
                    >
                      {hostEnvBusy ? <Loader2 className="h-4 w-4 animate-spin" /> : null}
                      {t("auto.ServerBookmarkSettingsModal_tsx.39")}
                    </button>
                  </div>
                </>
              ) : (
                <>
                  <p className="text-xs text-slate-400">
                    {t("auto.ServerBookmarkSettingsModal_tsx.40")}:{" "}
                    <code className="break-all text-amber-200/85">{envPath ?? t("auto.ServerBookmarkSettingsModal_tsx.41")}</code>
                    {envExists ? (
                      <span className="ml-2 text-emerald-400/90">{t("auto.ServerBookmarkSettingsModal_tsx.42")}</span>
                    ) : (
                      <span className="ml-2 text-slate-500">{t("auto.ServerBookmarkSettingsModal_tsx.43")}</span>
                    )}
                  </p>
                  <p className="text-xs text-slate-500">
                    {t("auto.ServerBookmarkSettingsModal_tsx.44")}{" "}
                    <code className="text-slate-400">run.sh</code> {t("auto.ServerBookmarkSettingsModal_tsx.45")}{" "}
                    <code className="text-slate-400">set -a; . ./app.env; set +a</code>),{" "}
                    {t("auto.ServerBookmarkSettingsModal_tsx.46")}
                  </p>
                  <textarea
                    value={envText}
                    onChange={(e) => {
                      setEnvText(e.target.value);
                      setEnvDirty(true);
                    }}
                    rows={14}
                    className="w-full rounded-xl border border-white/10 bg-black/35 px-3 py-2 font-mono text-xs text-slate-100 focus:border-amber-600/45 focus:outline-none"
                    spellCheck={false}
                  />
                  <div className="flex flex-wrap gap-2">
                    <button
                      type="button"
                      disabled={envBusy}
                      onClick={() => void loadAppEnv()}
                      className={`${btnBase} border border-white/15 bg-white/5 text-slate-200 hover:bg-white/10`}
                    >
                      {envBusy ? <Loader2 className="h-4 w-4 animate-spin" /> : null}
                      {t("auto.ServerBookmarkSettingsModal_tsx.47")}
                    </button>
                    <button
                      type="button"
                      disabled={envBusy || !envDirty}
                      onClick={() => void saveAppEnv()}
                      className={`${btnBase} bg-gradient-to-r from-red-700 to-red-900 text-white shadow-lg shadow-red-950/40 hover:brightness-110 disabled:opacity-40`}
                    >
                      {t("auto.ServerBookmarkSettingsModal_tsx.48")}
                    </button>
                  </div>
                </>
              )}
            </div>
          ) : null}

          {tab === "services" && sessionOk ? (
            <div className="space-y-3">
              <p className="text-xs leading-relaxed text-slate-500">
                {tr(
                  "Пакеты на хосте (Node, Python, nginx, СУБД): версии и systemd. Установка и удаление выполняются через sudo-скрипт на сервере (см. pirate-host-service.sh после обновления install.sh). Редактирование vhost — вкладка «nginx».",
                  "Packages on the host (Node, Python, nginx, databases): versions and systemd. Install/remove run via a sudo script on the server (see pirate-host-service.sh after updating install.sh). Vhost editing stays on the «nginx» tab.",
                )}
              </p>
              <HostServicesPanel sessionOk={sessionOk} />
            </div>
          ) : null}

          {tab === "antiddos" && sessionOk ? (
            <div className="space-y-3">
              <React.Suspense fallback={tabPanelFallback}>
                <AntiDdosPanel sessionOk={sessionOk} />
              </React.Suspense>
            </div>
          ) : null}

          {tab === "ssl" && sessionOk ? (
            <div className="space-y-3">
              <p className="text-xs leading-relaxed text-slate-500">
                {tr(
                  "Let’s Encrypt через gRPC: статус, выпуск, renew. Нужен активный gRPC, совпадающий с этой закладкой, и сопряжённый identity.",
                  "Let’s Encrypt over gRPC: status, issue, renew. Requires the active gRPC to match this bookmark and a paired identity.",
                )}
              </p>
              <React.Suspense fallback={tabPanelFallback}>
                <SslManagementPanel
                  grpcUrl={bookmark.url}
                  projectId={projectId}
                  controlBase={controlBase}
                  sessionOk={sessionOk}
                  sameServerAsActive={sameServerAsActive}
                  language={language === "ru" ? "ru" : "en"}
                  onHostRestartHint={setHostRestartHint}
                  onRestartPending={async () => {
                    setRestartPendingUntil(Date.now() + 90_000);
                    try {
                      await invoke("mark_control_api_recent_restart", { seconds: 90 });
                    } catch {
                      /* ignore */
                    }
                  }}
                />
              </React.Suspense>
            </div>
          ) : null}

          {tab === "process" && sessionOk ? (
            <div className="space-y-4">
              <div className="flex flex-wrap items-end gap-2">
                <div className="min-w-[8rem] flex-1">
                  <label className="mb-1 block text-xs text-slate-500">
                    {t("auto.ServerBookmarkSettingsModal_tsx.21")}
                  </label>
                  <input
                    value={projectId}
                    onChange={(e) => setProjectId(e.target.value)}
                    className="w-full rounded-lg border border-white/10 bg-black/30 px-3 py-2 font-mono text-sm text-slate-100 focus:outline-none"
                  />
                </div>
                <button
                  type="button"
                  disabled={projectsLoading}
                  onClick={() => void loadProjectsHint()}
                  className={`${btnBase} border border-white/15 bg-white/5 text-slate-200 hover:bg-white/10`}
                >
                  {projectsLoading ? <Loader2 className="h-4 w-4 animate-spin" /> : null}
                  {t("auto.ServerBookmarkSettingsModal_tsx.22")}
                </button>
                <button
                  type="button"
                  disabled={processStatusBusy}
                  onClick={() => void loadProcessStatus()}
                  className={`${btnBase} border border-red-800/40 bg-amber-950/30 text-amber-100 hover:bg-amber-950/50`}
                >
                  {processStatusBusy ? <Loader2 className="h-4 w-4 animate-spin" /> : null}
                  {t("auto.ServerBookmarkSettingsModal_tsx.23")}
                </button>
              </div>
              {processStatus ? (
                <p className="text-xs text-slate-400">
                  {tr("Версия", "Version")}:{" "}
                  <span className="font-mono text-amber-100/90">
                    {processStatus.current_version?.trim() || "—"}
                  </span>
                  {" · "}
                  {tr("Состояние", "State")}:{" "}
                  <span className="font-mono text-slate-200">{processStatus.state ?? "—"}</span>
                </p>
              ) : null}
              <p className="text-sm text-slate-400">
                <code className="text-orange-200/85">POST /api/v1/process/restart</code>{" "}
                {t("auto.ServerBookmarkSettingsModal_tsx.49")}
              </p>
              <div className="flex flex-wrap gap-2">
                <button
                  type="button"
                  disabled={restartBusy || stopBusy}
                  onClick={() => void restartProcess()}
                  className={`${btnBase} bg-gradient-to-r from-red-700 to-red-900 text-white shadow-lg shadow-red-950/40 hover:brightness-110 disabled:opacity-40`}
                >
                  {restartBusy ? <Loader2 className="h-4 w-4 animate-spin" /> : null}
                  {t("auto.ServerBookmarkSettingsModal_tsx.50")}
                </button>
                <button
                  type="button"
                  disabled={restartBusy || stopBusy}
                  onClick={() => void stopProcess()}
                  className={`${btnBase} border border-white/15 bg-white/5 text-slate-200 hover:bg-white/10 disabled:opacity-40`}
                >
                  {stopBusy ? <Loader2 className="h-4 w-4 animate-spin" /> : null}
                  {tr("Остановить", "Stop")}
                </button>
              </div>
              <CopyablePre
                value={restartOut}
                placeholder="—"
                className="rounded-xl border border-white/10 bg-black/40 p-3 text-xs text-slate-200"
                maxHeightClass="max-h-48"
              />
              <React.Suspense fallback={tabPanelFallback}>
                <ProcessListenersPanel
                  projectId={projectId}
                  controlBase={controlBase}
                  sessionOk={sessionOk}
                  language={language === "ru" ? "ru" : "en"}
                />
              </React.Suspense>
            </div>
          ) : null}

          {tab === "nginx" && sessionOk ? (
            <div className="relative space-y-4">
              {nginxMiniDialog ? (
                <div
                  className="absolute inset-0 z-20 flex items-center justify-center rounded-xl bg-black/70 p-3 backdrop-blur-[2px]"
                  role="dialog"
                  aria-modal="true"
                  aria-labelledby="nginx-mini-dialog-title"
                >
                  <div className="w-full max-w-sm rounded-xl border border-white/15 bg-[#0f0e0d] p-4 shadow-xl">
                    {nginxMiniDialog.kind === "domain" ? (
                      <>
                        <h3 id="nginx-mini-dialog-title" className="text-sm font-semibold text-slate-100">
                          {tr("Новый server_name", "New server_name")}
                        </h3>
                        <p className="mt-1 text-[11px] text-slate-500">
                          {tr("Один домен для первого блока server { } в этом файле.", "One domain for the first server { } block in this file.")}
                        </p>
                        <input
                          type="text"
                          value={nginxMiniDialog.draft}
                          onChange={(e) =>
                            setNginxMiniDialog({
                              kind: "domain",
                              path: nginxMiniDialog.path,
                              draft: e.target.value,
                            })
                          }
                          className="mt-3 w-full rounded-lg border border-white/10 bg-black/40 px-3 py-2 font-mono text-sm text-slate-100 focus:border-amber-600/45 focus:outline-none"
                          autoFocus
                        />
                        <div className="mt-3 flex justify-end gap-2">
                          <button
                            type="button"
                            className="rounded-lg border border-white/10 px-3 py-1.5 text-xs text-slate-300 hover:bg-white/5"
                            onClick={() => setNginxMiniDialog(null)}
                          >
                            {tr("Отмена", "Cancel")}
                          </button>
                          <button
                            type="button"
                            disabled={nginxSiteBusy}
                            className="rounded-lg border border-amber-800/40 bg-amber-950/40 px-3 py-1.5 text-xs font-medium text-amber-100 hover:bg-amber-950/60 disabled:opacity-50"
                            onClick={() => {
                              const d = nginxMiniDialog.draft.trim();
                              const p = nginxMiniDialog.path;
                              if (!d) return;
                              setNginxMiniDialog(null);
                              void runNginxAction({
                                action: "set_server_name",
                                path: p,
                                server_name: d,
                              });
                            }}
                          >
                            {tr("Применить", "Apply")}
                          </button>
                        </div>
                      </>
                    ) : (
                      <>
                        <h3 id="nginx-mini-dialog-title" className="text-sm font-semibold text-slate-100">
                          {nginxMiniDialog.enable
                            ? tr("Включить SSL?", "Enable SSL?")
                            : tr("Отключить SSL?", "Disable SSL?")}
                        </h3>
                        <p className="mt-1 text-[11px] leading-relaxed text-slate-500">
                          {nginxMiniDialog.enable
                            ? tr(
                                "Будут добавлены listen 443 ssl и пути к сертификату. Если PEM ещё нет, будет запущен certbot (нужны SSL_EMAIL или acme_email, SSL_MODE, sudo для certbot).",
                                "Adds listen 443 ssl and certificate paths. If the PEM is missing, certbot will run (set SSL_EMAIL or rely on request email, SSL_MODE, sudo for certbot).",
                              )
                            : tr(
                                "Будут удалены listen 443 / ssl_* в первом server-блоке.",
                                "Removes listen 443 / ssl_* in the first server block.",
                              )}
                        </p>
                        <div className="mt-3 flex justify-end gap-2">
                          <button
                            type="button"
                            className="rounded-lg border border-white/10 px-3 py-1.5 text-xs text-slate-300 hover:bg-white/5"
                            onClick={() => setNginxMiniDialog(null)}
                          >
                            {tr("Отмена", "Cancel")}
                          </button>
                          <button
                            type="button"
                            disabled={nginxSiteBusy}
                            className="rounded-lg border border-amber-800/40 bg-amber-950/40 px-3 py-1.5 text-xs font-medium text-amber-100 hover:bg-amber-950/60 disabled:opacity-50"
                            onClick={() => {
                              const { path, enable } = nginxMiniDialog;
                              setNginxMiniDialog(null);
                              const site = nginxSitesPayload?.sites.find((s) => s.path === path);
                              const post_check_host = site?.domains?.find(
                                (d) => d && d !== "_" && !d.startsWith("*."),
                              );
                              void runNginxAction({
                                action: "set_ssl",
                                path,
                                ssl_enabled: enable,
                                ...(post_check_host ? { post_check_host } : {}),
                                ...(enable ? { issue_certificate_if_missing: true } : {}),
                              });
                            }}
                          >
                            {tr("Подтвердить", "Confirm")}
                          </button>
                        </div>
                      </>
                    )}
                  </div>
                </div>
              ) : null}
              <div className="rounded-xl border border-white/10 bg-black/25 p-3 text-sm text-slate-300">
                <p className="font-semibold text-slate-100">{t("auto.ServerBookmarkSettingsModal_tsx.51")}</p>
                <p className="mt-2 text-xs text-slate-400">
                  Установлен:{" "}
                  <span className={nginxStatus?.installed ? "text-emerald-300" : "text-rose-300"}>
                    {nginxStatus?.installed ? t("auto.ServerBookmarkSettingsModal_tsx.52") : t("auto.ServerBookmarkSettingsModal_tsx.53")}
                  </span>
                  {nginxStatus?.version ? ` (${nginxStatus.version})` : ""}
                </p>
                <p className="mt-1 text-xs text-slate-400">
                  systemd: <code className="text-slate-300">{nginxStatus?.systemd_active ?? "—"}</code>
                </p>
                <p className="mt-1 text-xs text-slate-400">
                  site: <code className="break-all text-amber-200/85">{nginxStatus?.site_config_path ?? "—"}</code>
                </p>
                <p className="mt-1 text-xs text-slate-500">
                  {t("auto.ServerBookmarkSettingsModal_tsx.54")}: {nginxStatus?.site_file_exists ? t("auto.ServerBookmarkSettingsModal_tsx.55") : t("auto.ServerBookmarkSettingsModal_tsx.56")}; enabled:{" "}
                  {nginxStatus?.site_enabled ? t("auto.ServerBookmarkSettingsModal_tsx.57") : t("auto.ServerBookmarkSettingsModal_tsx.58")}; ensure-script:{" "}
                  {nginxStatus?.ensure_script_present ? "ok" : t("auto.ServerBookmarkSettingsModal_tsx.59")}; apply-script:{" "}
                  {nginxStatus?.apply_site_script_present ? "ok" : t("auto.ServerBookmarkSettingsModal_tsx.60")}; ops:{" "}
                  {nginxStatus?.ops_script_present ? "ok" : "—"}
                </p>
                <p className="mt-2 text-xs text-slate-500">
                  {nginxInstalled
                    ? "nginx уже установлен — можно применить режим доступа: только API или API + UI."
                    : "nginx не установлен — установите и выберите режим доступа: только API или API + UI."}
                </p>
                {!allowApiWithUiMode ? (
                  <p className="mt-2 text-xs text-amber-300/90">
                    На сервере no-UI сборка stack: режим «API + UI» и UI-переменные окружения скрыты.
                  </p>
                ) : null}
              </div>

              <div className="flex flex-wrap gap-2">
                <button
                  type="button"
                  disabled={nginxEnsureBusy}
                  onClick={() => void ensureNginx("api_only")}
                  className={`${btnBase} border border-red-800/40 bg-amber-950/30 text-amber-100 hover:bg-amber-950/50`}
                >
                  {nginxEnsureBusy ? <Loader2 className="h-4 w-4 animate-spin" /> : null}
                  {nginxInstalled
                    ? "Применить настройки nginx (API only)"
                    : "Установить и запустить nginx (API only)"}
                </button>
                {allowApiWithUiMode ? (
                  <button
                    type="button"
                    disabled={nginxEnsureBusy}
                    onClick={() => void ensureNginx("with_ui")}
                    className={`${btnBase} border border-red-800/40 bg-amber-950/30 text-amber-100 hover:bg-amber-950/50`}
                  >
                    {nginxEnsureBusy ? <Loader2 className="h-4 w-4 animate-spin" /> : null}
                    {nginxInstalled
                      ? "Применить настройки nginx (API + UI)"
                      : "Установить и запустить nginx (API + UI)"}
                  </button>
                ) : null}
                {nginxInstalled ? (
                  <button
                    type="button"
                    disabled={nginxEnsureBusy}
                    onClick={() => {
                      if (!confirmRemoveNginx) {
                        setConfirmRemoveNginx(true);
                        return;
                      }
                      void ensureNginx("remove");
                    }}
                    className={`${btnBase} border border-rose-700/50 bg-rose-950/35 text-rose-100 hover:bg-rose-950/55`}
                  >
                    {nginxEnsureBusy ? <Loader2 className="h-4 w-4 animate-spin" /> : null}
                    {confirmRemoveNginx ? t("auto.ServerBookmarkSettingsModal_tsx.61") : t("auto.ServerBookmarkSettingsModal_tsx.62")}
                  </button>
                ) : null}
                {confirmRemoveNginx && nginxInstalled ? (
                  <button
                    type="button"
                    disabled={nginxEnsureBusy}
                    onClick={() => setConfirmRemoveNginx(false)}
                    className={`${btnBase} border border-white/15 bg-white/5 text-slate-200 hover:bg-white/10`}
                  >
                    {t("auto.ServerBookmarkSettingsModal_tsx.63")}
                  </button>
                ) : null}
                <button
                  type="button"
                  disabled={nginxSiteBusy}
                  onClick={() => {
                    void loadNginxStatus();
                    void loadNginxSite();
                    void loadNginxInventory();
                  }}
                  className={`${btnBase} border border-white/15 bg-white/5 text-slate-200 hover:bg-white/10`}
                >
                  {nginxSiteBusy ? <Loader2 className="h-4 w-4 animate-spin" /> : null}
                  {t("auto.ServerBookmarkSettingsModal_tsx.64")}
                </button>
                <button
                  type="button"
                  disabled={nginxSiteBusy}
                  onClick={() => void runNginxPreflight()}
                  className={`${btnBase} border border-amber-800/30 bg-amber-950/25 text-amber-100 hover:bg-amber-950/45`}
                >
                  {nginxSiteBusy ? <Loader2 className="h-4 w-4 animate-spin" /> : null}
                  {tr("Проверка конфликтов (preflight)", "Conflict check (preflight)")}
                </button>
                <button
                  type="button"
                  disabled={nginxSiteBusy}
                  onClick={() => void runNginxAction({ action: "validate" })}
                  className={`${btnBase} border border-white/15 bg-white/5 text-slate-200 hover:bg-white/10`}
                >
                  {tr("nginx -t (validate)", "nginx -t (validate)")}
                </button>
                <button
                  type="button"
                  disabled={nginxSiteBusy}
                  onClick={() => void runNginxAction({ action: "reload" })}
                  className={`${btnBase} border border-emerald-800/30 bg-emerald-950/25 text-emerald-100 hover:bg-emerald-950/45`}
                >
                  {tr("Перезагрузить nginx", "Reload nginx")}
                </button>
              </div>

              {nginxSitesPayload && !nginxSitesPayload.ok && nginxSitesPayload.nginx_test_output ? (
                <p className="text-xs text-amber-200/90">
                  {tr("Предупреждение: последний nginx -t на сервере не прошёл (см. вывод).", "Warning: last nginx -t on server failed (see output).")}{" "}
                  <code className="text-[10px] text-slate-400 break-all">
                    {nginxSitesPayload.nginx_test_output.slice(0, 400)}
                  </code>
                </p>
              ) : null}

              {nginxSitesPayload && nginxSitesPayload.global_warnings.length > 0 ? (
                <div className="rounded-xl border border-amber-800/30 bg-amber-950/15 p-3 text-xs text-amber-100/90">
                  <p className="font-semibold text-amber-200">
                    {tr("Предупреждения", "Warnings")}
                  </p>
                  <ul className="mt-1 list-inside list-disc space-y-0.5 text-[11px] text-amber-100/80">
                    {nginxSitesPayload.global_warnings.map((w) => (
                      <li key={w}>{w}</li>
                    ))}
                  </ul>
                </div>
              ) : null}

              {nginxSitesPayload && nginxSitesPayload.global_conflicts.length > 0 ? (
                <div className="rounded-xl border border-rose-800/40 bg-rose-950/20 p-3 text-xs text-rose-100/90">
                  <p className="font-semibold text-rose-200">
                    {tr("Конфликты (домены / дубликаты)", "Conflicts (domains / duplicates)")}
                  </p>
                  <ul className="mt-1 list-inside list-disc space-y-0.5 text-[11px]">
                    {nginxSitesPayload.global_conflicts.map((c) => (
                      <li key={c.message}>{c.message}</li>
                    ))}
                  </ul>
                </div>
              ) : null}

              {nginxPreflightBlockers.length > 0 ? (
                <div className="rounded-xl border border-rose-800/40 bg-rose-950/20 p-3 text-xs text-rose-100/90">
                  <p className="font-semibold text-rose-200">
                    {tr("Блокирующие проверки preflight", "Preflight blockers")}
                  </p>
                  <ul className="mt-1 list-inside list-disc space-y-0.5 text-[11px]">
                    {nginxPreflightBlockers.map((b) => (
                      <li key={b.message}>{b.message}</li>
                    ))}
                  </ul>
                </div>
              ) : null}

              {nginxSitesPayload ? (
                <>
                <div className="overflow-x-auto rounded-xl border border-white/10">
                  <table className="min-w-full border-collapse text-left text-[11px] text-slate-200">
                    <thead className="bg-black/40 text-slate-400">
                      <tr>
                        <th className="border-b border-white/10 px-2 py-2 font-medium">
                          {tr("Файл / путь", "File / path")}
                        </th>
                        <th className="border-b border-white/10 px-2 py-2 font-medium">kind</th>
                        <th className="border-b border-white/10 px-2 py-2 font-medium">
                          {tr("Активен", "Active")}
                        </th>
                        <th className="border-b border-white/10 px-2 py-2 font-medium">managed</th>
                        <th className="border-b border-white/10 px-2 py-2 font-medium">SSL</th>
                        <th className="border-b border-white/10 px-2 py-2 font-medium">UI</th>
                        <th className="border-b border-white/10 px-2 py-2 font-medium">
                          {tr("Домены", "Domains")}
                        </th>
                        <th className="border-b border-white/10 px-2 py-2 font-medium">
                          {tr("Действия", "Actions")}
                        </th>
                      </tr>
                    </thead>
                    <tbody>
                      {nginxSitesPayload.sites.length === 0 ? (
                        <tr>
                          <td colSpan={8} className="px-2 py-4 text-center text-slate-500">
                            {tr("Нет файлов в известных каталогах.", "No files found in scanned paths.")}
                          </td>
                        </tr>
                      ) : null}
                      {nginxSitesPayload.sites.map((row) => (
                        <NginxSiteTableRow
                          key={row.site_id}
                          row={row}
                          highlighted={nginxFileEditor?.path === row.path}
                          busy={nginxSiteBusy}
                          onOpenFile={openNginxFileEditor}
                          onAction={runNginxAction}
                          onMiniDialog={setNginxMiniDialog}
                        />
                      ))}
                    </tbody>
                  </table>
                </div>
                {nginxFileEditor ? (
                  <div className="mt-3 rounded-xl border border-cyan-800/40 bg-[#0c0b0a] p-4 space-y-3">
                    <div className="flex flex-wrap items-start justify-between gap-3">
                      <div className="min-w-0 flex-1">
                        <p className="text-xs font-semibold text-slate-200">
                          {tr("Файл nginx", "Nginx file")}
                        </p>
                        <code className="mt-1 block break-all text-[10px] leading-snug text-amber-200/85">
                          {nginxFileEditor.path}
                        </code>
                      </div>
                      <div className="flex flex-wrap items-center gap-2">
                        <button
                          type="button"
                          disabled={nginxSiteBusy || nginxFileEditor.loading}
                          onClick={() => reloadNginxFileEditor()}
                          className={`${btnBase} border border-white/15 bg-white/5 px-3 py-1.5 text-xs text-slate-200 hover:bg-white/10`}
                        >
                          {nginxSiteBusy || nginxFileEditor.loading ? (
                            <Loader2 className="h-3.5 w-3.5 animate-spin" />
                          ) : null}
                          {tr("Обновить с сервера", "Reload from server")}
                        </button>
                        <button
                          type="button"
                          disabled={
                            nginxSiteBusy ||
                            nginxFileEditor.loading ||
                            nginxFileEditor.readOnly ||
                            !nginxFileEditor.dirty
                          }
                          onClick={() => void saveNginxInventoryFile()}
                          className={`${btnBase} border border-emerald-800/35 bg-emerald-950/30 px-3 py-1.5 text-xs text-emerald-100 hover:bg-emerald-950/50 disabled:opacity-40`}
                        >
                          {nginxSiteBusy ? <Loader2 className="h-3.5 w-3.5 animate-spin" /> : null}
                          {tr("Сохранить", "Save")}
                        </button>
                        <button
                          type="button"
                          disabled={nginxSiteBusy}
                          onClick={() => setNginxFileEditor(null)}
                          className={`${btnBase} border border-white/10 bg-transparent px-3 py-1.5 text-xs text-slate-400 hover:bg-white/5`}
                        >
                          {tr("Закрыть", "Close")}
                        </button>
                      </div>
                    </div>
                    {nginxFileEditor.readOnlyReason ? (
                      <p className="text-[11px] leading-relaxed text-amber-200/90">{nginxFileEditor.readOnlyReason}</p>
                    ) : null}
                    {nginxFileEditor.loading ? (
                      <div className="flex items-center gap-2 py-8 text-sm text-slate-500">
                        <Loader2 className="h-5 w-5 animate-spin shrink-0" />
                        {tr("Загрузка…", "Loading…")}
                      </div>
                    ) : (
                      <textarea
                        value={nginxFileEditor.content}
                        readOnly={nginxFileEditor.readOnly}
                        onChange={(e) =>
                          setNginxFileEditor((prev) =>
                            prev ? { ...prev, content: e.target.value, dirty: true } : null,
                          )
                        }
                        rows={18}
                        spellCheck={false}
                        className="w-full rounded-xl border border-white/10 bg-black/40 px-3 py-2 font-mono text-[11px] leading-relaxed text-slate-100 focus:border-cyan-700/45 focus:outline-none read-only:opacity-80"
                      />
                    )}
                  </div>
                ) : null}
                </>
              ) : !nginxSiteBusy ? (
                <p className="text-xs text-slate-500">
                  {tr(
                    "Список конфигов недоступен: обновите control-api на сервере (нужен GET /api/v1/nginx/sites) или нажмите «Обновить».",
                    "Config list unavailable: upgrade control-api (GET /api/v1/nginx/sites) or press refresh.",
                  )}
                </p>
              ) : null}

              <details className="rounded-xl border border-white/10 bg-black/20 p-3">
                <summary className="cursor-pointer text-sm text-slate-200">
                  {tr("Расширенно: сырой vhost (Pirate site)", "Advanced: raw vhost (Pirate site file)")}
                </summary>
                <p className="mt-2 text-xs text-slate-500">
                  {tr("Путь", "Path")}: <code className="text-slate-400">{nginxSitePath ?? "—"}</code>
                </p>
                <textarea
                  value={nginxSiteText}
                  onChange={(e) => {
                    setNginxSiteText(e.target.value);
                    setNginxSiteDirty(true);
                  }}
                  rows={10}
                  className="mt-2 w-full rounded-xl border border-white/10 bg-black/35 px-3 py-2 font-mono text-xs text-slate-100 focus:border-amber-600/45 focus:outline-none"
                  spellCheck={false}
                />
                <div className="mt-2 flex flex-wrap gap-2">
                  <button
                    type="button"
                    disabled={nginxSiteBusy || !nginxSiteDirty}
                    onClick={() => void saveNginxSite()}
                    className={`${btnBase} bg-gradient-to-r from-red-700 to-red-900 text-white shadow-lg shadow-red-950/40 hover:brightness-110 disabled:opacity-40`}
                  >
                    {nginxSiteBusy ? <Loader2 className="h-4 w-4 animate-spin" /> : null}
                    {t("auto.ServerBookmarkSettingsModal_tsx.65")}
                  </button>
                </div>
              </details>
              <CopyablePre
                value={nginxOut}
                placeholder="—"
                className="rounded-xl border border-white/10 bg-black/40 p-3 text-xs text-slate-200"
                maxHeightClass="max-h-48"
              />
              {nginxEnvUpdate ? (
                <div className="rounded-xl border border-emerald-800/30 bg-emerald-950/20 p-3 text-xs text-emerald-100/90">
                  <p className="font-semibold text-emerald-200">
                    {t("auto.ServerBookmarkSettingsModal_tsx.66")} ({nginxEnvUpdate.mode ?? "nginx"})
                  </p>
                  <p className="mt-1 text-emerald-100/80">
                    restart_scheduled: {nginxEnvUpdate.restart_scheduled ? "true" : "false"}
                  </p>
                  {nginxEnvUpdate.updates?.length ? (
                    <ul className="mt-2 space-y-1">
                      {nginxEnvUpdate.updates.map((u, i) => (
                        <li key={`${u.key ?? "key"}-${i}`} className="font-mono text-[11px]">
                          {(u.key ?? "KEY") + ": "}
                          {u.old_value ?? "∅"} {" -> "} {u.new_value ?? "∅"}
                        </li>
                      ))}
                    </ul>
                  ) : null}
                </div>
              ) : null}
            </div>
          ) : null}

          {!sessionOk && tab !== "connect" ? (
            <p className="text-sm text-slate-500">{t("auto.ServerBookmarkSettingsModal_tsx.67")}</p>
          ) : null}
        </div>
      </div>
      </ModalDialog>

      {nginxProgressOpen ? (
        <ModalDialog
          open
          zClassName="z-modalBlocking"
          closeOnBackdrop={false}
          onClose={() => {
            if (nginxCancelRequested) return;
            nginxOpSeq.current += 1;
            setNginxCancelRequested(true);
            setNginxEnsureBusy(false);
            setNginxProgressOpen(false);
            if (nginxProgressTimer.current !== null) {
              window.clearInterval(nginxProgressTimer.current);
              nginxProgressTimer.current = null;
            }
          }}
          panelClassName="w-full max-w-md"
        >
          <div className="rounded-2xl border border-white/10 bg-[#0a0908] p-4 shadow-2xl shadow-black/60">
            <h3 className="text-sm font-semibold text-slate-100">{nginxProgressTitle}</h3>
            <p className="mt-1 text-xs text-slate-400">
              {nginxCancelRequested
                ? t("auto.ServerBookmarkSettingsModal_tsx.68")
                : t("auto.ServerBookmarkSettingsModal_tsx.69")}
            </p>
            <p className="mt-2 text-[11px] leading-snug text-slate-500">
              {tr(
                "Серверная операция может продолжаться после закрытия этого окна.",
                "The server-side operation may continue after you dismiss this panel.",
              )}
            </p>
            <div className="mt-3 h-2 w-full overflow-hidden rounded-full bg-white/10">
              <div
                className="h-full rounded-full bg-gradient-to-r from-red-700 to-red-900 transition-[width] duration-300"
                style={{ width: `${Math.max(0, Math.min(100, nginxProgressValue))}%` }}
              />
            </div>
            <p className="mt-2 text-right text-[11px] text-slate-500">{nginxProgressValue}%</p>
            <div className="mt-3 flex justify-end gap-2">
              <button
                type="button"
                data-modal-initial-focus
                disabled={nginxCancelRequested}
                onClick={() => {
                  nginxOpSeq.current += 1;
                  setNginxCancelRequested(true);
                  setNginxEnsureBusy(false);
                  setNginxProgressOpen(false);
                  if (nginxProgressTimer.current !== null) {
                    window.clearInterval(nginxProgressTimer.current);
                    nginxProgressTimer.current = null;
                  }
                }}
                className={`${btnBase} border border-white/15 bg-white/5 px-3 py-1.5 text-xs text-slate-200 hover:bg-white/10`}
              >
                {tr("Скрыть окно", "Dismiss")}
              </button>
            </div>
          </div>
        </ModalDialog>
      ) : null}
    </>
  );
}
