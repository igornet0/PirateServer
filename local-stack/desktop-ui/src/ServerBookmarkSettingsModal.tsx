/**
 * Настройки удалённого сервера через control-api (JWT): статус, окружение (host env + app.env), перезапуск процесса.
 * Требует входа в control-api; base URL задаётся для этого сохранённого gRPC URL.
 */
import { invoke } from "@tauri-apps/api/core";
import { AlertCircle, Loader2, Settings, X } from "lucide-react";
import React, { useCallback, useEffect, useMemo, useRef, useState } from "react";
import { suggestControlApiFromGrpcUrl } from "./controlApiUrl";
import { HostServerEnvPanel } from "./serverDeployEnv/HostServerEnvPanel";
import { AntiDdosPanel } from "./AntiDdosPanel";
import { HostServicesPanel } from "./HostServicesPanel";
import { useI18n } from "./i18n";
import { CopyablePre } from "./ui/CopyablePre";
import { ModalDialog } from "./ui/ModalDialog";

const btnBase =
  "inline-flex items-center justify-center gap-2 rounded-xl px-4 py-2.5 text-sm font-semibold transition-all duration-200 focus-visible:outline-none focus-visible:ring-2 focus-visible:ring-red-600/80 focus-visible:ring-offset-2 focus-visible:ring-offset-[#050204] active:scale-[0.98] disabled:pointer-events-none disabled:opacity-50";

export type ServerBookmark = {
  id: string;
  label: string;
  url: string;
};

function normalizeGrpcUrl(s: string): string {
  return s.trim().replace(/\/+$/, "");
}

function waitForNextFrame(): Promise<void> {
  return new Promise((resolve) => requestAnimationFrame(() => resolve()));
}

type TabId = "connect" | "info" | "env" | "services" | "antiddos" | "nginx" | "process";

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
  const tr = (ru: string, en: string) => (language === "ru" ? ru : en);
    const [tab, setTab] = useState<TabId>("connect");
  const [listLabelDraft, setListLabelDraft] = useState("");
  const [listLabelBusy, setListLabelBusy] = useState(false);
  const [listLabelErr, setListLabelErr] = useState<string | null>(null);
  const [controlBase, setControlBase] = useState("");
  const [user, setUser] = useState("");
  const [pass, setPass] = useState("");
  const [loginBusy, setLoginBusy] = useState(false);
  const [sessionOk, setSessionOk] = useState(false);
  const [err, setErr] = useState<string | null>(null);

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

  const [restartBusy, setRestartBusy] = useState(false);
  const [restartOut, setRestartOut] = useState<string | null>(null);

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
  }, [open, prefillBase, sameServerAsActive, bookmark.label]);

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

  const loadProjectsHint = useCallback(async () => {
    setProjectsLoading(true);
    setErr(null);
    try {
      await invoke("set_control_api_base", { url: controlBase.trim() });
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

  const onLogin = async () => {
    setLoginBusy(true);
    setErr(null);
    try {
      await invoke("set_control_api_base", { url: controlBase.trim() });
      await invoke("control_api_login", {
        baseUrl: controlBase.trim(),
        username: user.trim(),
        password: pass,
      });
      setPass("");
      setSessionOk(true);
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
      await invoke("set_control_api_base", { url: controlBase.trim() });
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
      await invoke("set_control_api_base", { url: controlBase.trim() });
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
      await invoke("set_control_api_base", { url: controlBase.trim() });
      const raw = await invoke<string>("control_api_fetch_host_deploy_env_json");
      const parsed = JSON.parse(raw) as { path?: string; content?: string; exists?: boolean };
      setHostEnvPath(typeof parsed.path === "string" ? parsed.path : null);
      setHostEnvExists(Boolean(parsed.exists));
      setHostEnvText(typeof parsed.content === "string" ? parsed.content : "");
      setHostEnvDirty(false);
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
      await invoke("set_control_api_base", { url: controlBase.trim() });
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
      await invoke("set_control_api_base", { url: controlBase.trim() });
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
      await invoke("set_control_api_base", { url: controlBase.trim() });
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

  const restartProcess = async () => {
    setRestartBusy(true);
    setErr(null);
    setRestartOut(null);
    try {
      await invoke("set_control_api_base", { url: controlBase.trim() });
      const raw = await invoke<string>("control_api_restart_process_json", { projectId });
      setRestartOut(raw);
    } catch (e) {
      setErr(String(e));
    } finally {
      setRestartBusy(false);
    }
  };

  const loadNginxStatus = useCallback(async () => {
    setNginxSiteBusy(true);
    setErr(null);
    try {
      await invoke("set_control_api_base", { url: controlBase.trim() });
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
      await invoke("set_control_api_base", { url: controlBase.trim() });
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

  const saveNginxSite = async () => {
    setNginxSiteBusy(true);
    setErr(null);
    setNginxOut(null);
    try {
      await invoke("set_control_api_base", { url: controlBase.trim() });
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
      await invoke("set_control_api_base", { url: controlBase.trim() });
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
      } catch {
        setNginxEnvUpdate(null);
      }
      setNginxProgressValue(100);
      await loadNginxStatus();
      await loadNginxSite();
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
  }, [open, tab, sessionOk, loadNginxStatus, loadNginxSite]);

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
    { id: "info", label: t("auto.ServerBookmarkSettingsModal_tsx.5") },
    { id: "env", label: t("auto.ServerBookmarkSettingsModal_tsx.6") },
    { id: "services", label: tr("Сервисы", "Services") },
    { id: "antiddos", label: tr("Anti-DDoS", "Anti-DDoS") },
    { id: "nginx", label: "nginx" },
    { id: "process", label: t("auto.ServerBookmarkSettingsModal_tsx.7") },
  ];
  const nginxInstalled = Boolean(nginxStatus?.installed);

  const goTab = (id: TabId) => {
    setErr(null);
    setTab(id);
  };

  return (
    <>
      <ModalDialog
        open={open}
        onClose={onClose}
        zClassName="z-modalServerSettings"
        closeOnBackdrop={false}
        closeOnEscape={!nginxProgressOpen}
        panelClassName="w-full max-w-2xl max-h-[90vh] min-h-0"
        aria-labelledby="srv-settings-title"
      >
        <div className="max-h-[90vh] w-full overflow-hidden rounded-2xl border border-white/10 bg-[#0a0908] shadow-2xl shadow-black/60">
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
            </div>
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
              <AntiDdosPanel sessionOk={sessionOk} />
            </div>
          ) : null}

          {tab === "process" && sessionOk ? (
            <div className="space-y-4">
              <p className="text-sm text-slate-400">
                <code className="text-orange-200/85">POST /api/v1/process/restart</code>{" "}
                {t("auto.ServerBookmarkSettingsModal_tsx.49")}
              </p>
              <button
                type="button"
                disabled={restartBusy}
                onClick={() => void restartProcess()}
                className={`${btnBase} bg-gradient-to-r from-red-700 to-red-900 text-white shadow-lg shadow-red-950/40 hover:brightness-110 disabled:opacity-40`}
              >
                {restartBusy ? <Loader2 className="h-4 w-4 animate-spin" /> : null}
                {t("auto.ServerBookmarkSettingsModal_tsx.50")}
              </button>
              <CopyablePre
                value={restartOut}
                placeholder="—"
                className="rounded-xl border border-white/10 bg-black/40 p-3 text-xs text-slate-200"
                maxHeightClass="max-h-48"
              />
            </div>
          ) : null}

          {tab === "nginx" && sessionOk ? (
            <div className="space-y-4">
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
                  {nginxStatus?.apply_site_script_present ? "ok" : t("auto.ServerBookmarkSettingsModal_tsx.60")}
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
                  }}
                  className={`${btnBase} border border-white/15 bg-white/5 text-slate-200 hover:bg-white/10`}
                >
                  {nginxSiteBusy ? <Loader2 className="h-4 w-4 animate-spin" /> : null}
                  {t("auto.ServerBookmarkSettingsModal_tsx.64")}
                </button>
              </div>

              <p className="text-xs text-slate-500">
                Конфиг сайта nginx: <code className="text-slate-400">{nginxSitePath ?? "—"}</code>
              </p>
              <textarea
                value={nginxSiteText}
                onChange={(e) => {
                  setNginxSiteText(e.target.value);
                  setNginxSiteDirty(true);
                }}
                rows={14}
                className="w-full rounded-xl border border-white/10 bg-black/35 px-3 py-2 font-mono text-xs text-slate-100 focus:border-amber-600/45 focus:outline-none"
                spellCheck={false}
              />
              <div className="flex flex-wrap gap-2">
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
