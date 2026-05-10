import { useVirtualizer } from "@tanstack/react-virtual";
import { ChevronRight } from "lucide-react";
import React, { useCallback, useEffect, useMemo, useRef } from "react";
import { rowKeysForGrid } from "./hostDbApi";

const ROW_PX = 26;

type Col = { name: string; type?: string };

type Props = {
  columns: Col[];
  rows: unknown[];
  /** Open JSON/detail drawer for row */
  onRowDetail: (index: number) => void;
  onNeedMore?: () => void;
  busy?: boolean;
};

function cellText(cell: unknown): string {
  if (cell === null || cell === undefined) return "—";
  if (typeof cell === "object") return JSON.stringify(cell);
  return String(cell);
}

export function DbDataGrid({ columns, rows, onRowDetail, onNeedMore, busy }: Props) {
  const parentRef = useRef<HTMLDivElement>(null);
  const colKeys = useMemo(
    () => (columns.length > 0 ? columns.map((c) => c.name) : rowKeysForGrid(rows)),
    [columns, rows],
  );

  const virtualizer = useVirtualizer({
    count: rows.length,
    getScrollElement: () => parentRef.current,
    estimateSize: () => ROW_PX,
    overscan: 16,
  });

  const onScroll = useCallback(() => {
    const el = parentRef.current;
    if (!el || !onNeedMore || busy) return;
    if (el.scrollHeight - el.scrollTop - el.clientHeight < 120) {
      onNeedMore();
    }
  }, [onNeedMore, busy]);

  useEffect(() => {
    const el = parentRef.current;
    if (!el) return;
    el.addEventListener("scroll", onScroll, { passive: true });
    return () => el.removeEventListener("scroll", onScroll);
  }, [onScroll]);

  const gridTemplate = useMemo(() => {
    const parts = ["2rem", ...colKeys.map(() => "minmax(8rem,1fr)")];
    return parts.join(" ");
  }, [colKeys]);

  return (
    <div ref={parentRef} className="min-h-0 flex-1 overflow-auto rounded border border-red-900/20 bg-black/20">
      <div className="min-w-max">
        <div
          className="sticky top-0 z-[1] grid border-b border-red-900/30 bg-slate-900/95 text-[10px] font-medium text-slate-400"
          style={{ gridTemplateColumns: gridTemplate }}
        >
          <div className="px-1 py-1.5" />
          {colKeys.map((k) => (
            <div key={k} className="truncate px-2 py-1.5">
              {k}
            </div>
          ))}
        </div>
        <div
          className="relative w-full"
          style={{ height: `${virtualizer.getTotalSize()}px` }}
        >
          {virtualizer.getVirtualItems().map((v) => {
            const row = rows[v.index];
            const obj = row && typeof row === "object" && !Array.isArray(row) ? (row as Record<string, unknown>) : null;
            return (
              <div
                key={v.key}
                className="absolute left-0 grid w-full items-center border-b border-white/[0.06] text-[11px] text-slate-200 hover:bg-red-950/15"
                style={{
                  height: `${v.size}px`,
                  transform: `translateY(${v.start}px)`,
                  gridTemplateColumns: gridTemplate,
                }}
              >
                <button
                  type="button"
                  title="Details"
                  className="flex h-full items-center justify-center border-r border-white/5 text-slate-500 hover:text-amber-200/90"
                  onClick={() => onRowDetail(v.index)}
                >
                  <ChevronRight className="h-3.5 w-3.5" />
                </button>
                {colKeys.map((k) => (
                  <div key={k} className="truncate px-2 py-0.5 font-mono tabular-nums">
                    {obj && k in obj ? cellText(obj[k]) : "—"}
                  </div>
                ))}
              </div>
            );
          })}
        </div>
      </div>
      {rows.length === 0 && !busy ? (
        <p className="p-4 text-center text-[11px] text-slate-600">—</p>
      ) : null}
    </div>
  );
}
