import { invoke } from "@tauri-apps/api/core";
import { Loader2, Skull, X } from "lucide-react";
import React, { useCallback, useEffect, useMemo, useState } from "react";
import { useDebouncedValue } from "./hooks/useDebouncedValue";
import { useControlApiSession } from "./session/ControlApiSession";

export type ListenerRow = {
  port: number;
  protocol: string;
  bind: string;
  pid: number;
  ppid?: number | null;
  user: string;
  cmdline: string;
  scope: string;
  managed_by_project: boolean;
};

type Props = {
  projectId: string;
  controlBase: string;
  sessionOk: boolean;
  language: "ru" | "en";
};

function tr(language: "ru" | "en", ru: string, en: string) {
  return language === "ru" ? ru : en;
}

function isElevationRequired(err: string): boolean {
  return err.includes("elevation_required") || err.includes("permission denied killing");
}

export function ProcessListenersPanel({ projectId, controlBase, sessionOk, language }: Props) {
  const { ensureControlApiBase } = useControlApiSession();
  const [scope, setScope] = useState<"project" | "all">("project");
  const [rows, setRows] = useState<ListenerRow[]>([]);
  const [busy, setBusy] = useState(false);
  const [killBusyPid, setKillBusyPid] = useState<number | null>(null);
  const [err, setErr] = useState<string | null>(null);
  const [filterPort, setFilterPort] = useState("");
  const [filterPid, setFilterPid] = useState("");
  const [filterCmd, setFilterCmd] = useState("");
  const debouncedPort = useDebouncedValue(filterPort, 300);
  const debouncedPid = useDebouncedValue(filterPid, 300);
  const debouncedCmd = useDebouncedValue(filterCmd, 300);
  const [rootModal, setRootModal] = useState<{
    pid: number;
    port: number;
    signal: "TERM" | "KILL";
    allowForeign: boolean;
  } | null>(null);
  const [rootPassword, setRootPassword] = useState("");

  const loadListeners = useCallback(async () => {
    if (!sessionOk || !controlBase.trim()) return;
    setBusy(true);
    setErr(null);
    try {
      await ensureControlApiBase(controlBase.trim());
      const raw = await invoke<string>("control_api_fetch_process_listeners_json", {
        projectId,
        scope,
      });
      const parsed = JSON.parse(raw) as { rows?: ListenerRow[] };
      setRows(Array.isArray(parsed.rows) ? parsed.rows : []);
    } catch (e) {
      setRows([]);
      setErr(String(e));
    } finally {
      setBusy(false);
    }
  }, [controlBase, projectId, scope, sessionOk]);

  useEffect(() => {
    void loadListeners();
  }, [loadListeners]);

  const filtered = useMemo(() => {
    const p = debouncedPort.trim();
    const pid = debouncedPid.trim();
    const cmd = debouncedCmd.trim().toLowerCase();
    return rows.filter((r) => {
      if (p && !String(r.port).includes(p)) return false;
      if (pid && !String(r.pid).includes(pid)) return false;
      if (cmd && !r.cmdline.toLowerCase().includes(cmd) && !r.user.toLowerCase().includes(cmd)) {
        return false;
      }
      return true;
    });
  }, [rows, debouncedPort, debouncedPid, debouncedCmd]);

  const runKill = async (
    pid: number,
    port: number,
    signal: "TERM" | "KILL",
    password?: string,
    allowForeign = false,
  ) => {
    if (pid <= 0) return;
    setKillBusyPid(pid);
    setErr(null);
    try {
      await ensureControlApiBase(controlBase.trim());
      await invoke<string>("control_api_kill_process_listener_json", {
        projectId,
        pid,
        signal,
        port: port > 0 ? port : null,
        rootPassword: password ?? null,
        allowForeign,
      });
      setRootModal(null);
      setRootPassword("");
      await loadListeners();
    } catch (e) {
      const msg = String(e);
      if (isElevationRequired(msg) && !password) {
        setRootModal({ pid, port, signal, allowForeign });
        setRootPassword("");
      } else {
        setErr(msg);
      }
    } finally {
      setKillBusyPid(null);
    }
  };

  const canKillRow = (r: ListenerRow) => r.pid > 0;

  return (
    <div className="space-y-3">
      <div className="flex flex-wrap items-end gap-2">
        <div className="flex rounded-lg border border-white/10 p-0.5">
          <button
            type="button"
            onClick={() => setScope("project")}
            className={`rounded-md px-3 py-1 text-xs font-medium ${
              scope === "project"
                ? "bg-amber-900/50 text-amber-100"
                : "text-slate-400 hover:text-slate-200"
            }`}
          >
            {tr(language, "Порты проекта", "Project ports")}
          </button>
          <button
            type="button"
            onClick={() => setScope("all")}
            className={`rounded-md px-3 py-1 text-xs font-medium ${
              scope === "all"
                ? "bg-amber-900/50 text-amber-100"
                : "text-slate-400 hover:text-slate-200"
            }`}
          >
            {tr(language, "Все слушатели", "All listeners")}
          </button>
        </div>
        <button
          type="button"
          disabled={busy}
          onClick={() => void loadListeners()}
          className="rounded-lg border border-white/15 bg-white/5 px-3 py-1.5 text-xs text-slate-200 hover:bg-white/10 disabled:opacity-50"
        >
          {busy ? <Loader2 className="inline h-3.5 w-3.5 animate-spin" /> : null}{" "}
          {tr(language, "Обновить", "Refresh")}
        </button>
      </div>

      <div className="grid gap-2 sm:grid-cols-3">
        <input
          type="text"
          value={filterPort}
          onChange={(e) => setFilterPort(e.target.value)}
          placeholder={tr(language, "Фильтр: порт", "Filter: port")}
          className="rounded-lg border border-white/10 bg-black/30 px-2 py-1.5 text-xs text-slate-100"
        />
        <input
          type="text"
          value={filterPid}
          onChange={(e) => setFilterPid(e.target.value)}
          placeholder={tr(language, "Фильтр: PID", "Filter: PID")}
          className="rounded-lg border border-white/10 bg-black/30 px-2 py-1.5 text-xs text-slate-100"
        />
        <input
          type="text"
          value={filterCmd}
          onChange={(e) => setFilterCmd(e.target.value)}
          placeholder={tr(language, "Фильтр: команда", "Filter: command")}
          className="rounded-lg border border-white/10 bg-black/30 px-2 py-1.5 text-xs text-slate-100"
        />
      </div>

      {err ? <p className="text-xs text-red-300">{err}</p> : null}

      <div className="overflow-x-auto rounded-xl border border-white/10">
        <table className="w-full min-w-[32rem] text-left text-xs">
          <thead className="bg-black/40 text-slate-500">
            <tr>
              <th className="px-2 py-2">{tr(language, "Порт", "Port")}</th>
              <th className="px-2 py-2">{tr(language, "Привязка", "Bind")}</th>
              <th className="px-2 py-2">PID</th>
              <th className="px-2 py-2">{tr(language, "Пользователь", "User")}</th>
              <th className="px-2 py-2">{tr(language, "Команда", "Command")}</th>
              <th className="w-28 px-2 py-2">{tr(language, "Действия", "Actions")}</th>
            </tr>
          </thead>
          <tbody className="divide-y divide-white/5 text-slate-200">
            {filtered.length === 0 ? (
              <tr>
                <td colSpan={6} className="px-3 py-6 text-center text-slate-500">
                  {busy
                    ? tr(language, "Загрузка…", "Loading…")
                    : tr(language, "Нет слушателей", "No listeners")}
                </td>
              </tr>
            ) : (
              filtered.map((r) => {
                const key = `${r.port}-${r.bind}-${r.pid}-${r.protocol}`;
                const killable = canKillRow(r);
                const foreign = scope === "all" && !r.managed_by_project;
                return (
                  <tr key={key} className={r.managed_by_project ? "bg-amber-950/15" : undefined}>
                    <td className="px-2 py-1.5 font-mono">
                      {r.port}
                      {r.managed_by_project ? (
                        <span className="ml-1 text-[10px] text-amber-400/80">●</span>
                      ) : null}
                    </td>
                    <td
                      className="max-w-[6rem] truncate px-2 py-1.5 font-mono text-slate-400"
                      title={r.bind}
                    >
                      {r.bind}
                    </td>
                    <td className="px-2 py-1.5 font-mono">{r.pid || "—"}</td>
                    <td className="px-2 py-1.5">{r.user || "—"}</td>
                    <td className="max-w-[14rem] truncate px-2 py-1.5" title={r.cmdline}>
                      {r.cmdline || "—"}
                    </td>
                    <td className="px-2 py-1.5">
                      <div className="flex flex-col gap-1">
                        <button
                          type="button"
                          disabled={!killable || killBusyPid === r.pid}
                          title={
                            foreign
                              ? tr(
                                  language,
                                  "Чужой процесс — потребуется пароль root",
                                  "Foreign process — root password required",
                                )
                              : undefined
                          }
                          onClick={() => void runKill(r.pid, r.port, "TERM", undefined, foreign)}
                          className="rounded border border-white/10 px-2 py-0.5 text-[10px] hover:bg-white/5 disabled:opacity-30"
                        >
                          {killBusyPid === r.pid ? (
                            <Loader2 className="inline h-3 w-3 animate-spin" />
                          ) : null}{" "}
                          SIGTERM
                        </button>
                        <button
                          type="button"
                          disabled={!killable || killBusyPid === r.pid}
                          onClick={() => void runKill(r.pid, r.port, "KILL", undefined, foreign)}
                          className="rounded border border-red-900/40 px-2 py-0.5 text-[10px] text-red-200 hover:bg-red-950/30 disabled:opacity-30"
                        >
                          <Skull className="inline h-3 w-3" /> SIGKILL
                        </button>
                      </div>
                    </td>
                  </tr>
                );
              })
            )}
          </tbody>
        </table>
      </div>

      {rootModal ? (
        <div
          className="fixed inset-0 z-[90] flex items-center justify-center bg-black/65 p-4"
          role="dialog"
          aria-modal="true"
        >
          <div className="w-full max-w-sm rounded-xl border border-white/15 bg-[#0f0e0d] p-4 shadow-xl">
            <div className="flex items-start justify-between gap-2">
              <p className="text-sm font-semibold text-slate-100">
                {tr(language, "Пароль root", "Root password")}
              </p>
              <button
                type="button"
                className="text-slate-500 hover:text-slate-300"
                onClick={() => {
                  setRootModal(null);
                  setRootPassword("");
                }}
              >
                <X className="h-4 w-4" />
              </button>
            </div>
            <p className="mt-1 text-[11px] leading-relaxed text-slate-500">
              {tr(
                language,
                `Нет прав завершить PID ${rootModal.pid}. Введите пароль root (su на сервере). Пароль не сохраняется.`,
                `Cannot kill PID ${rootModal.pid} without elevation. Enter root password (su on server). Password is not stored.`,
              )}
            </p>
            <input
              type="password"
              autoComplete="off"
              value={rootPassword}
              onChange={(e) => setRootPassword(e.target.value)}
              className="mt-3 w-full rounded-lg border border-white/10 bg-black/40 px-3 py-2 text-sm text-slate-100"
              autoFocus
            />
            <div className="mt-3 flex justify-end gap-2">
              <button
                type="button"
                className="rounded-lg border border-white/10 px-3 py-1.5 text-xs text-slate-300"
                onClick={() => {
                  setRootModal(null);
                  setRootPassword("");
                }}
              >
                {tr(language, "Отмена", "Cancel")}
              </button>
              <button
                type="button"
                disabled={!rootPassword.trim() || killBusyPid != null}
                className="rounded-lg border border-red-800/50 bg-red-950/40 px-3 py-1.5 text-xs font-medium text-red-100 disabled:opacity-40"
                onClick={() => {
                  const pw = rootPassword;
                  setRootPassword("");
                  void runKill(
                    rootModal.pid,
                    rootModal.port,
                    rootModal.signal,
                    pw,
                    rootModal.allowForeign,
                  );
                }}
              >
                {tr(language, "Завершить", "Kill")}
              </button>
            </div>
          </div>
        </div>
      ) : null}
    </div>
  );
}
