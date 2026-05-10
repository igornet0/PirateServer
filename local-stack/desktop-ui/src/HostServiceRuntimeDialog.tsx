import { invoke } from "@tauri-apps/api/core";
import { Loader2, Plus, Power, Save, X } from "lucide-react";
import React, { useCallback, useEffect, useState } from "react";
import { toast } from "sonner";
import { isSensitiveHostEnvKey, SecretFieldRow } from "./hostServiceSecretFields";

const btnSm =
  "inline-flex items-center justify-center gap-1.5 rounded-lg px-3 py-2 text-xs font-semibold transition focus-visible:outline-none focus-visible:ring-2 focus-visible:ring-amber-600/60 disabled:opacity-50";
const fieldClass =
  "w-full min-w-0 rounded-lg border border-white/10 bg-black/30 px-2.5 py-1.5 font-mono text-[11px] text-slate-100 placeholder:text-slate-600 focus:border-amber-600/40 focus:outline-none";

type Row = { k: string; v: string };

type Props = {
  serviceId: string;
  displayName: string;
  onClose: () => void;
  onAfterChange: () => void;
  tr: (ru: string, en: string) => string;
};

export function HostServiceRuntimeDialog({ serviceId, displayName, onClose, onAfterChange, tr }: Props) {
  const [loading, setLoading] = useState(true);
  const [saving, setSaving] = useState(false);
  const [restarting, setRestarting] = useState(false);
  const [rows, setRows] = useState<Row[]>([]);
  const [loadErr, setLoadErr] = useState<string | null>(null);

  const toRows = (env: Record<string, string>): Row[] => {
    const e = { ...env };
    return Object.keys(e)
      .sort()
      .map((k) => ({ k, v: e[k] ?? "" }));
  };

  const load = useCallback(async () => {
    setLoading(true);
    setLoadErr(null);
    try {
      const j = await invoke<string>("control_api_host_service_runtime_get_json", { id: serviceId });
      const p = JSON.parse(j) as { env?: Record<string, string> };
      setRows(toRows(p.env && typeof p.env === "object" ? p.env : {}));
    } catch (e) {
      const msg = String(e);
      setLoadErr(msg);
      setRows([]);
    } finally {
      setLoading(false);
    }
  }, [serviceId]);

  useEffect(() => {
    void load();
  }, [load]);

  const buildEnv = (): Record<string, string> => {
    const out: Record<string, string> = {};
    for (const { k, v } of rows) {
      const key = k.trim();
      if (!key) continue;
      out[key] = v;
    }
    return out;
  };

  const onSave = async () => {
    const env = buildEnv();
    if (Object.keys(env).length === 0) {
      toast.error(
        tr("Нет переменных", "No variables"),
        { description: tr("Добавьте хотя бы одну строку с непустым именем ключа.", "Add at least one line with a non-empty key name.") },
      );
      return;
    }
    setSaving(true);
    try {
      const body = JSON.stringify({ env });
      await invoke<string>("control_api_host_service_runtime_put_json", {
        id: serviceId,
        bodyJson: body,
      });
      toast.success(tr("Сохранено", "Saved"));
      onAfterChange();
      await load();
    } catch (e) {
      toast.error(tr("Сохранение не удалось", "Save failed"), { description: String(e) });
    } finally {
      setSaving(false);
    }
  };

  const onRestartOnly = async () => {
    setRestarting(true);
    try {
      await invoke<string>("control_api_host_service_restart", { id: serviceId });
      toast.success(tr("Перезапуск выполнен", "Restart completed"));
      onAfterChange();
      await load();
    } catch (e) {
      toast.error(tr("Перезапуск не удался", "Restart failed"), { description: String(e) });
    } finally {
      setRestarting(false);
    }
  };

  return (
    <div className="fixed inset-0 z-modalNestedHigh flex items-center justify-center bg-black/60 p-4">
      <div
        className="flex max-h-[min(90vh,720px)] w-full max-w-2xl flex-col rounded-xl border border-amber-900/30 bg-[#0c0a08] p-4 shadow-xl"
        role="dialog"
        aria-labelledby="host-runtime-title"
      >
        <div className="mb-3 flex items-start justify-between gap-2">
          <div>
            <h2 id="host-runtime-title" className="text-sm font-semibold text-slate-100">
              {tr("Параметры", "Settings")}
              {displayName ? <span className="ml-2 text-amber-200/90">{displayName}</span> : null}
            </h2>
            <p className="mt-1 text-[11px] text-slate-500">
              {tr("Файл окружения на хосте (minio / meilisearch), затем по желанию перезапуск unit.", "Host env file; restart the unit if needed.")}
            </p>
            <p className="mt-0.5 font-mono text-[10px] text-slate-600">{serviceId}</p>
          </div>
          <button
            type="button"
            onClick={onClose}
            className="rounded-lg p-1.5 text-slate-500 hover:bg-white/5 hover:text-slate-300"
            aria-label={tr("Закрыть", "Close")}
          >
            <X className="h-4 w-4" />
          </button>
        </div>

        {loadErr ? (
          <p className="mb-2 rounded-lg border border-red-900/40 bg-red-950/25 px-3 py-2 text-xs text-red-200/90">{loadErr}</p>
        ) : null}

        {loading ? (
          <div className="flex flex-1 items-center justify-center py-12 text-slate-500">
            <Loader2 className="h-8 w-8 animate-spin" />
          </div>
        ) : (
          <div className="min-h-0 flex-1 space-y-2 overflow-y-auto pr-0.5">
            {rows.length === 0 && !loadErr ? (
              <p className="text-xs text-slate-500">
                {tr("Файл пуст или не прочитан. Добавьте переменные ниже и сохраните.", "File is empty or unread. Add variables below and save.")}
              </p>
            ) : null}
            {rows.map((row, i) => (
              <div key={`${row.k}-${i}`} className="flex flex-wrap items-center gap-1.5">
                <input
                  type="text"
                  className={fieldClass + " flex-1 min-w-[8rem]"}
                  value={row.k}
                  placeholder="KEY"
                  onChange={(e) => {
                    const t = e.target.value;
                    setRows((prev) => prev.map((p, j) => (j === i ? { ...p, k: t } : p)));
                  }}
                />
                <span className="text-slate-600">=</span>
                {isSensitiveHostEnvKey(row.k) ? (
                  <div className="flex min-w-0 flex-[2] basis-[10rem]">
                    <SecretFieldRow
                      value={row.v}
                      onChange={(t) => {
                        setRows((prev) => prev.map((p, j) => (j === i ? { ...p, v: t } : p)));
                      }}
                      tr={tr}
                      inputClassName={fieldClass + " min-w-0 flex-1"}
                      placeholder="value"
                    />
                  </div>
                ) : (
                  <input
                    type="text"
                    className={fieldClass + " min-w-0 flex-[2] basis-[10rem]"}
                    value={row.v}
                    placeholder="value"
                    onChange={(e) => {
                      const t = e.target.value;
                      setRows((prev) => prev.map((p, j) => (j === i ? { ...p, v: t } : p)));
                    }}
                  />
                )}
                <button
                  type="button"
                  onClick={() => setRows((prev) => prev.filter((_, j) => j !== i))}
                  className="shrink-0 rounded-lg border border-white/10 bg-white/5 px-2 py-1.5 text-[10px] text-slate-500 hover:text-red-300"
                >
                  {tr("Убрать", "Remove")}
                </button>
              </div>
            ))}
            <button
              type="button"
              onClick={() => setRows((prev) => [...prev, { k: "", v: "" }])}
              className={`${btnSm} border border-white/10 bg-white/5 text-slate-300`}
            >
              <Plus className="h-3.5 w-3.5" />
              {tr("Добавить переменную", "Add variable")}
            </button>
          </div>
        )}

        <div className="mt-4 flex flex-wrap items-center justify-end gap-2 border-t border-white/10 pt-3">
          <button type="button" onClick={onClose} className={`${btnSm} border border-white/10 bg-white/5`}>
            {tr("Закрыть", "Close")}
          </button>
          <button
            type="button"
            disabled={loading || saving || loadErr != null}
            onClick={() => void onSave()}
            className={`${btnSm} border border-amber-800/50 bg-amber-950/40 text-amber-100`}
          >
            {saving ? <Loader2 className="h-3.5 w-3.5 animate-spin" /> : <Save className="h-3.5 w-3.5" />}
            {tr("Сохранить", "Save")}
          </button>
          <button
            type="button"
            disabled={loading || restarting || loadErr != null}
            onClick={() => void onRestartOnly()}
            className={`${btnSm} border border-slate-700/50 bg-slate-900/50 text-slate-200`}
            title={tr("Только systemctl restart (без записи файла)", "systemctl restart only (no file write)")}
          >
            {restarting ? <Loader2 className="h-3.5 w-3.5 animate-spin" /> : <Power className="h-3.5 w-3.5" />}
            {tr("Перезапуск", "Restart")}
          </button>
        </div>
      </div>
    </div>
  );
}
