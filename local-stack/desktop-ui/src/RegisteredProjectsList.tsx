/**
 * Local registry from `pirate-projects.json` (name in pirate.toml → folder path).
 */
import { invoke } from "@tauri-apps/api/core";
import { AlertTriangle, FolderInput, FolderOpen, Loader2, RefreshCw, Trash2 } from "lucide-react";
import React, { useCallback, useEffect, useState } from "react";
import type { RegisteredProject } from "./registered-projects-types";
import { useI18n } from "./i18n";

const btnSm =
  "inline-flex items-center justify-center gap-1.5 rounded-lg px-2.5 py-1.5 text-xs font-semibold transition focus-visible:outline-none focus-visible:ring-2 focus-visible:ring-red-600/80 disabled:pointer-events-none disabled:opacity-50";

function truncatePath(path: string, max = 56): string {
  if (path.length <= max) return path;
  const head = Math.floor(max / 2) - 1;
  const tail = max - head - 3;
  return `${path.slice(0, head)}…${path.slice(-tail)}`;
}

export function RegisteredProjectsList({
  refreshKey,
  currentDeployDir,
  onSelectPath,
  onRegistryChanged,
  variant = "full",
}: {
  refreshKey: number;
  currentDeployDir: string | null;
  onSelectPath: (path: string) => void;
  onRegistryChanged?: () => void;
  /** `compact` — боковая колонка: плотные строки без длинных подсказок */
  variant?: "full" | "compact";
}) {
  const { language, t } = useI18n();
  const tr = (ru: string, en: string) => (language === "ru" ? ru : en);
    const compact = variant === "compact";
  const [items, setItems] = useState<RegisteredProject[]>([]);
  const [loading, setLoading] = useState(true);
  const [err, setErr] = useState<string | null>(null);
  const [busy, setBusy] = useState<string | null>(null);

  const load = useCallback(async () => {
    setLoading(true);
    setErr(null);
    try {
      const rows = await invoke<RegisteredProject[]>("list_registered_projects");
      setItems(rows);
    } catch (e) {
      setErr(String(e));
      setItems([]);
    } finally {
      setLoading(false);
    }
  }, []);

  useEffect(() => {
    void load();
  }, [refreshKey, load]);

  const onAddCurrent = async () => {
    const p = currentDeployDir?.trim();
    if (!p) return;
    setBusy("add");
    setErr(null);
    try {
      await invoke<string>("register_project_from_directory", { path: p });
      onRegistryChanged?.();
      await load();
    } catch (e) {
      setErr(String(e));
    } finally {
      setBusy(null);
    }
  };

  const onRemove = async (name: string) => {
    setBusy(`rm:${name}`);
    setErr(null);
    try {
      await invoke<boolean>("remove_registered_project", { name });
      onRegistryChanged?.();
      await load();
    } catch (e) {
      setErr(String(e));
    } finally {
      setBusy(null);
    }
  };

  return (
    <div className={compact ? "space-y-2" : "space-y-3"}>
      <div className={`flex flex-wrap items-center gap-2 ${compact ? "justify-between" : "justify-between"}`}>
        {!compact ? (
          <p className="text-xs text-slate-500">
            <code className="text-orange-200/85">[project].name</code> — для списка;{" "}
            <code className="text-orange-200/85">[project].version</code>{" "}
            {tr(
              "— версия манифеста (сравнивается с сервером после деплоя). Слот деплоя на сервере подбирается автоматически; при необходимости укажите",
              "— manifest version (compared with server after deploy). Deployment slot on server is selected automatically; if needed set",
            )}{" "}
            <code className="text-orange-200/85">[project].deploy_project_id</code>{" "}
            {t("auto.RegisteredProjectsList_tsx.1")} <code className="text-orange-200/85">pirate.toml</code>
            {t("auto.RegisteredProjectsList_tsx.2")}
          </p>
        ) : null}
        <div className={`flex flex-wrap gap-1.5 ${compact ? "w-full" : ""}`}>
          <button
            type="button"
            disabled={loading}
            onClick={() => void load()}
            title={t("auto.RegisteredProjectsList_tsx.3")}
            className={`${btnSm} border border-border-subtle bg-white/[0.04] text-slate-200 ${compact ? "px-2 py-1" : ""}`}
          >
            {loading ? <Loader2 className="h-3.5 w-3.5 animate-spin" /> : <RefreshCw className="h-3.5 w-3.5" />}
            {!compact ? t("auto.RegisteredProjectsList_tsx.4") : null}
          </button>
          <button
            type="button"
            disabled={!currentDeployDir?.trim() || busy !== null}
            onClick={() => void onAddCurrent()}
            title={t("auto.RegisteredProjectsList_tsx.5")}
            className={`${btnSm} border border-red-900/45 bg-red-950/35 text-orange-100 ${compact ? "flex-1 px-2 py-1" : ""}`}
          >
            <FolderInput className="h-3.5 w-3.5" />
            {compact ? t("auto.RegisteredProjectsList_tsx.6") : t("auto.RegisteredProjectsList_tsx.7")}
          </button>
        </div>
      </div>

      {err ? <p className="text-sm text-rose-300">{err}</p> : null}

      {loading && items.length === 0 ? (
        <p className="flex items-center gap-2 text-sm text-slate-500">
          <Loader2 className="h-4 w-4 animate-spin" />
          {t("auto.RegisteredProjectsList_tsx.8")}
        </p>
      ) : null}

      {!loading && items.length === 0 ? (
        <p
          className={`rounded-lg border border-border-subtle bg-panel-raised text-slate-400 ${
            compact ? "px-2 py-2 text-[11px] leading-snug" : "rounded-xl px-4 py-3 text-sm"
          }`}
        >
          {t("auto.RegisteredProjectsList_tsx.9")}
          <code className="text-orange-200/85">pirate.toml</code>
          {!compact ? t("auto.RegisteredProjectsList_tsx.10") : "."}
        </p>
      ) : null}

      {items.length > 0 ? (
        <ul
          className={`divide-y divide-border-subtle border border-border-subtle bg-panel-raised ${
            compact ? "rounded-lg text-[11px]" : "rounded-xl"
          }`}
        >
          {items.map((row) => {
            const active =
              currentDeployDir &&
              row.path.replace(/\/$/, "") === currentDeployDir.replace(/\/$/, "");
            return (
              <li
                key={row.name}
                className={`flex flex-wrap items-center justify-between gap-1.5 px-2 py-2 ${
                  active ? "bg-red-950/25 ring-1 ring-inset ring-red-600/35" : ""
                } ${row.needsDeploy && !active ? "bg-red-950/22" : ""}`}
              >
                <div className="min-w-0 flex-1">
                  <div className="flex items-center gap-1.5">
                    <p className="truncate font-medium text-slate-200">{row.name}</p>
                    {row.needsDeploy ? (
                      <span title={t("auto.RegisteredProjectsList_tsx.11")}>
                        <AlertTriangle className="h-3 w-3 shrink-0 text-orange-400" aria-hidden />
                      </span>
                    ) : null}
                  </div>
                  <p className="truncate text-slate-500" title={row.path}>
                    {compact ? truncatePath(row.path, 36) : truncatePath(row.path)}
                  </p>
                  {!compact ? (
                    <>
                      <div className="mt-1 flex flex-wrap gap-x-3 gap-y-0.5 text-[11px] text-slate-500">
                        <span>
                          {t("auto.RegisteredProjectsList_tsx.12")}{" "}
                          <code className="text-slate-300">{row.localVersion.trim() || "—"}</code>
                        </span>
                        <span>
                          {t("auto.RegisteredProjectsList_tsx.13")}{" "}
                          <code className="text-slate-300">
                            {!row.connected ? t("auto.RegisteredProjectsList_tsx.14") : row.serverProjectVersion.trim() || "—"}
                          </code>
                        </span>
                        <span title={t("auto.RegisteredProjectsList_tsx.15")}>
                          id: <code className="text-slate-400">{row.deployProjectId}</code>
                        </span>
                      </div>
                      {row.needsDeploy ? (
                        <p className="mt-1.5 flex items-center gap-1.5 text-xs font-medium text-orange-200/95">
                          <AlertTriangle className="h-3.5 w-3.5 shrink-0" />
                          <code className="text-orange-100/90">[project].version</code>{" "}
                          {tr(
                            "не совпадает с сервером — выберите проект и задеплойте, либо обновите манифест.",
                            "does not match server version — select this project and deploy, or update manifest.",
                          )}
                        </p>
                      ) : null}
                      {!row.localVersion.trim() ? (
                        <p className="mt-1 text-[11px] text-slate-500">
                          {t("auto.RegisteredProjectsList_tsx.16")}
                          <code className="text-orange-200/85">[project].version</code>
                          {t("auto.RegisteredProjectsList_tsx.17")}
                        </p>
                      ) : null}
                    </>
                  ) : (
                    <p className="mt-0.5 text-[10px] text-slate-500">
                      <span className="text-slate-400">v</span>
                      {row.localVersion.trim() || "—"}
                      <span className="mx-1 text-slate-600">·</span>
                      {!row.connected ? "offline" : row.serverProjectVersion.trim() || "—"}
                    </p>
                  )}
                </div>
                <div className={`flex shrink-0 flex-wrap ${compact ? "gap-1" : "gap-2"}`}>
                  <button
                    type="button"
                    title={t("auto.RegisteredProjectsList_tsx.18")}
                    onClick={() => {
                      void (async () => {
                        setErr(null);
                        try {
                          await invoke("open_project_folder", { path: row.path });
                        } catch (e) {
                          setErr(String(e));
                        }
                      })();
                    }}
                    className={`${btnSm} border border-border-subtle bg-white/[0.04] text-slate-200 ${compact ? "p-1.5" : ""}`}
                  >
                    <FolderOpen className="h-3.5 w-3.5" />
                    {!compact ? t("auto.RegisteredProjectsList_tsx.19") : null}
                  </button>
                  <button
                    type="button"
                    onClick={() => onSelectPath(row.path)}
                    className={`${btnSm} bg-gradient-to-r from-red-800 to-red-900 text-white hover:from-red-700 hover:to-red-800 shadow-glow ${compact ? "px-2 py-1" : ""}`}
                  >
                    {t("auto.RegisteredProjectsList_tsx.20")}
                  </button>
                  <button
                    type="button"
                    disabled={busy !== null}
                    onClick={() => void onRemove(row.name)}
                    className={`${btnSm} border border-rose-500/30 text-rose-200 ${compact ? "p-1.5" : ""}`}
                    title={t("auto.RegisteredProjectsList_tsx.21")}
                  >
                    {busy === `rm:${row.name}` ? (
                      <Loader2 className="h-3.5 w-3.5 animate-spin" />
                    ) : (
                      <Trash2 className="h-3.5 w-3.5" />
                    )}
                  </button>
                </div>
              </li>
            );
          })}
        </ul>
      ) : null}
    </div>
  );
}
