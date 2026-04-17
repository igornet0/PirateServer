/**
 * Host software inventory (GET /api/v1/host-services) and install/remove via control-api.
 */
import { invoke } from "@tauri-apps/api/core";
import { Loader2, RefreshCw } from "lucide-react";
import React, { useCallback, useEffect, useState } from "react";
import { toast } from "sonner";
import { useI18n } from "./i18n";

const btnSm =
  "inline-flex items-center justify-center gap-1.5 rounded-lg px-2.5 py-1.5 text-xs font-semibold transition focus-visible:outline-none focus-visible:ring-2 focus-visible:ring-red-600/80 disabled:pointer-events-none disabled:opacity-50";

type HostServiceRow = {
  id: string;
  display_name: string;
  category: string;
  installed: boolean;
  version?: string | null;
  running?: boolean | null;
  systemd_unit?: string | null;
  actions: string;
  notes?: string | null;
};

type HostServicesView = {
  services: HostServiceRow[];
  cifs_mounts: string[];
  dispatch_script_present: boolean;
};

export function HostServicesPanel({ sessionOk }: { sessionOk: boolean }) {
  const { language } = useI18n();
  const tr = (ru: string, en: string) => (language === "ru" ? ru : en);
  const [data, setData] = useState<HostServicesView | null>(null);
  const [loading, setLoading] = useState(false);
  const [busyId, setBusyId] = useState<string | null>(null);
  const [out, setOut] = useState<string | null>(null);
  const [confirmRemoveId, setConfirmRemoveId] = useState<string | null>(null);

  const load = useCallback(async () => {
    setLoading(true);
    setOut(null);
    try {
      const j = await invoke<string>("control_api_fetch_host_services_json");
      setData(JSON.parse(j) as HostServicesView);
    } catch (e) {
      setData(null);
      setOut(String(e));
    } finally {
      setLoading(false);
    }
  }, []);

  useEffect(() => {
    if (sessionOk) void load();
  }, [sessionOk, load]);

  const runInstall = async (id: string) => {
    setBusyId(id);
    setOut(null);
    try {
      const r = await invoke<string>("control_api_host_service_install", { id });
      setOut(r);
      toast.success(tr("Готово", "Done"), {
        description: r.length > 200 ? `${r.slice(0, 200)}…` : r,
      });
      await load();
    } catch (e) {
      const msg = String(e);
      setOut(msg);
      toast.error(tr("Ошибка установки", "Install failed"), { description: msg });
    } finally {
      setBusyId(null);
    }
  };

  const runRemove = async (id: string) => {
    setConfirmRemoveId(null);
    setBusyId(id);
    setOut(null);
    try {
      const r = await invoke<string>("control_api_host_service_remove", { id });
      setOut(r);
      toast.success(tr("Готово", "Done"), {
        description: r.length > 200 ? `${r.slice(0, 200)}…` : r,
      });
      await load();
    } catch (e) {
      const msg = String(e);
      setOut(msg);
      toast.error(tr("Ошибка удаления", "Remove failed"), { description: msg });
    } finally {
      setBusyId(null);
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
        <button
          type="button"
          disabled={loading}
          onClick={() => void load()}
          className={`${btnSm} border border-white/10 bg-white/5 text-slate-200 hover:bg-white/10`}
        >
          {loading ? <Loader2 className="h-3.5 w-3.5 animate-spin" /> : <RefreshCw className="h-3.5 w-3.5" />}
          {tr("Обновить", "Refresh")}
        </button>
        {data && !data.dispatch_script_present ? (
          <span className="text-xs text-amber-200/90">
            {tr(
              "Скрипт pirate-host-service.sh не найден на сервере — обновите server-stack (install.sh).",
              "pirate-host-service.sh not found on server — update server-stack (install.sh).",
            )}
          </span>
        ) : null}
      </div>

      {out ? (
        <pre className="max-h-40 overflow-auto whitespace-pre-wrap rounded-lg border border-white/10 bg-black/30 p-3 font-mono text-[11px] text-slate-300">
          {out}
        </pre>
      ) : null}

      {data?.cifs_mounts?.length ? (
        <div className="rounded-lg border border-white/10 bg-black/20 p-3 text-xs text-slate-400">
          <span className="font-semibold text-slate-300">{tr("CIFS монтирования:", "CIFS mounts:")}</span>{" "}
          {data.cifs_mounts.join(", ")}
        </div>
      ) : null}

      <div className="overflow-x-auto rounded-xl border border-white/10">
        <table className="w-full min-w-[640px] text-left text-xs">
          <thead className="border-b border-white/10 bg-black/30 text-slate-500">
            <tr>
              <th className="px-3 py-2 font-medium">{tr("Сервис", "Service")}</th>
              <th className="px-3 py-2 font-medium">{tr("Категория", "Category")}</th>
              <th className="px-3 py-2 font-medium">{tr("Версия", "Version")}</th>
              <th className="px-3 py-2 font-medium">{tr("Статус", "Status")}</th>
              <th className="px-3 py-2 font-medium">{tr("Действия", "Actions")}</th>
            </tr>
          </thead>
          <tbody>
            {(data?.services ?? []).map((row) => (
              <tr key={row.id} className="border-b border-white/5 text-slate-300">
                <td className="px-3 py-2">
                  <div className="font-medium text-slate-200">{row.display_name}</div>
                  <div className="font-mono text-[10px] text-slate-500">{row.id}</div>
                  {row.notes ? <p className="mt-1 text-[10px] leading-snug text-slate-500">{row.notes}</p> : null}
                </td>
                <td className="px-3 py-2 capitalize text-slate-400">{row.category}</td>
                <td className="px-3 py-2 font-mono text-[11px]">{row.version ?? "—"}</td>
                <td className="px-3 py-2">
                  {row.running === undefined || row.running === null
                    ? row.systemd_unit
                      ? "—"
                      : "—"
                    : row.running
                      ? tr("запущен", "running")
                      : tr("остановлен", "stopped")}
                  {row.systemd_unit ? (
                    <span className="ml-1 font-mono text-[10px] text-slate-500">({row.systemd_unit})</span>
                  ) : null}
                </td>
                <td className="px-3 py-2">
                  <div className="flex flex-wrap gap-1">
                    {row.actions === "install" && data?.dispatch_script_present ? (
                      <button
                        type="button"
                        disabled={busyId !== null}
                        onClick={() => void runInstall(row.id)}
                        className={`${btnSm} border border-emerald-800/40 bg-emerald-950/30 text-emerald-100`}
                      >
                        {busyId === row.id ? <Loader2 className="h-3 w-3 animate-spin" /> : null}
                        {tr("Установить", "Install")}
                      </button>
                    ) : null}
                    {row.actions === "remove" && data?.dispatch_script_present ? (
                      <button
                        type="button"
                        disabled={busyId !== null}
                        onClick={() => setConfirmRemoveId(row.id)}
                        className={`${btnSm} border border-red-800/40 bg-red-950/30 text-red-100`}
                      >
                        {busyId === row.id ? <Loader2 className="h-3 w-3 animate-spin" /> : null}
                        {tr("Удалить", "Remove")}
                      </button>
                    ) : null}
                    {row.actions === "none" ? (
                      <span className="text-slate-600">—</span>
                    ) : null}
                  </div>
                </td>
              </tr>
            ))}
          </tbody>
        </table>
      </div>

      {loading && !data ? (
        <div className="flex justify-center py-8 text-slate-500">
          <Loader2 className="h-6 w-6 animate-spin" />
        </div>
      ) : null}

      {confirmRemoveId ? (
        <div className="fixed inset-0 z-modalNestedHigh flex items-center justify-center bg-black/60 p-4">
          <div className="max-w-md rounded-xl border border-red-900/40 bg-[#120808] p-4 shadow-xl">
            <p className="text-sm text-slate-200">
              {tr(
                "Удалить пакеты этого сервиса на хосте? Для баз данных это может уничтожить данные.",
                "Remove this service’s packages on the host? For databases this may destroy data.",
              )}
            </p>
            <p className="mt-2 font-mono text-xs text-amber-200/90">{confirmRemoveId}</p>
            <div className="mt-4 flex flex-wrap gap-2">
              <button
                type="button"
                className={`${btnSm} border border-red-700/50 bg-red-950/40 text-red-100`}
                onClick={() => void runRemove(confirmRemoveId)}
              >
                {tr("Подтвердить удаление", "Confirm remove")}
              </button>
              <button type="button" className={`${btnSm} border border-white/10 bg-white/5`} onClick={() => setConfirmRemoveId(null)}>
                {tr("Отмена", "Cancel")}
              </button>
            </div>
          </div>
        </div>
      ) : null}
    </div>
  );
}
