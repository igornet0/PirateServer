import { createColumnHelper, flexRender, getCoreRowModel, useReactTable } from "@tanstack/react-table";
import uPlot from "uplot";
import "uplot/dist/uPlot.min.css";
import React, { useEffect, useMemo, useRef } from "react";
import { useI18n } from "../i18n";
import type { QueryResult } from "./dbExplorerStore";

const colHelper = createColumnHelper<Record<string, unknown>>();

function MiniGrid({ result, title }: { result: QueryResult; title: string }) {
  const cols = useMemo(
    () =>
      result.columns.map((c) =>
        colHelper.accessor((row) => (row as Record<string, unknown>)[c] ?? null, {
          id: c,
          header: c,
          cell: (i) => String(i.getValue() ?? ""),
        }),
      ),
    [result.columns],
  );
  const data = useMemo(() => result.rows as Record<string, unknown>[], [result.rows]);
  const t = useReactTable({ data, columns: cols, getCoreRowModel: getCoreRowModel() });
  return (
    <div className="mb-3">
      <p className="mb-1 text-[10px] font-medium text-slate-400">{title}</p>
      <div className="max-h-48 overflow-auto rounded border border-red-900/20 bg-black/20">
        <table className="w-full text-left text-[10px] text-slate-200">
          <thead>
            {t.getHeaderGroups().map((hg) => (
              <tr key={hg.id}>
                {hg.headers.map((h) => (
                  <th key={h.id} className="border-b border-white/5 px-1 py-0.5">
                    {flexRender(h.column.columnDef.header, h.getContext())}
                  </th>
                ))}
              </tr>
            ))}
          </thead>
          <tbody>
            {t.getRowModel().rows.map((row) => (
              <tr key={row.id} className="border-b border-white/5">
                {row.getVisibleCells().map((cell) => (
                  <td key={cell.id} className="px-1 py-0.5">
                    {flexRender(cell.column.columnDef.cell, cell.getContext())}
                  </td>
                ))}
              </tr>
            ))}
          </tbody>
        </table>
      </div>
    </div>
  );
}

function DbSizeUplot({ result }: { result: QueryResult }) {
  const el = useRef<HTMLDivElement | null>(null);
  useEffect(() => {
    const node = el.current;
    if (!node) return;
    const rows = result.rows as Record<string, unknown>[];
    const y = rows.map((r) => {
      const v = r.size_bytes ?? r.sizeBytes;
      return typeof v === "number" ? v : parseInt(String(v ?? 0), 10);
    });
    if (y.length === 0) return;
    const x = y.map((_, i) => i);
    const data: uPlot.AlignedData = [x, y];
    const inst = new uPlot(
      {
        width: node.clientWidth || 360,
        height: 120,
        series: [{}, { label: "size_bytes", stroke: "#c45c3e" }],
        scales: { x: { time: false } },
      },
      data,
      node,
    );
    return () => {
      inst.destroy();
    };
  }, [result]);
  return <div ref={el} className="mb-2 w-full min-h-[120px] overflow-hidden rounded border border-red-900/20 bg-black/30" />;
}

/** Renders `PgStatsBundle` JSON from the desktop backend. */
export function PgStatsCharts({ raw }: { raw: string }) {
  const { language } = useI18n();
  const tr = (ru: string, en: string) => (language === "ru" ? ru : en);
  let payload: {
    sourceNote?: string;
    rttMs?: number;
    databaseSizes?: QueryResult;
    connectionSummary?: QueryResult;
    topActivity?: QueryResult;
    statementsExt?: QueryResult;
    error?: string;
  };
  try {
    payload = JSON.parse(raw) as typeof payload;
  } catch {
    return <p className="text-[10px] text-rose-300">Invalid stats JSON</p>;
  }
  if (payload.error) {
    return <p className="text-[10px] text-rose-300">{payload.error}</p>;
  }
  return (
    <div className="max-h-full overflow-auto pr-1 text-[10px] text-slate-200">
      <p className="mb-2 border-b border-amber-900/20 pb-2 text-amber-100/90">{payload.sourceNote}</p>
      {typeof payload.rttMs === "number" ? (
        <p className="mb-2 font-mono text-slate-300">
          RTT: {payload.rttMs} ms — {tr("пульс `SELECT 1`", "heartbeat `SELECT 1`")}
        </p>
      ) : null}
      {payload.databaseSizes && payload.databaseSizes.rows.length > 0 ? (
        <DbSizeUplot result={payload.databaseSizes} />
      ) : null}
      {payload.databaseSizes ? <MiniGrid result={payload.databaseSizes} title={tr("Размеры БД", "Database sizes")} /> : null}
      {payload.connectionSummary ? (
        <MiniGrid result={payload.connectionSummary} title={tr("Сводка соединений", "Connections")} />
      ) : null}
      {payload.topActivity ? <MiniGrid result={payload.topActivity} title={tr("Активность", "Activity")} /> : null}
      {payload.statementsExt ? (
        <MiniGrid result={payload.statementsExt} title={tr("pg_stat_statements (если расширение)", "pg_stat_statements (if ext)")} />
      ) : (
        <p className="text-slate-500">{tr("pg_stat_statements недоступен (расширение не создано).", "pg_stat_statements unavailable (extension).")}</p>
      )}
    </div>
  );
}
