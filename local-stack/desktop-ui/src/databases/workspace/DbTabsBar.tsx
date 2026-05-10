import { FileCode2, Pin, Table2, X } from "lucide-react";
import React, { useCallback, useRef } from "react";

export type DbTabBarItem = {
  id: string;
  title: string;
  pinned?: boolean;
  kind: "table_data" | "table_schema" | "sql" | "admin" | string;
};

type Props = {
  tabs: DbTabBarItem[];
  activeId: string | null;
  onActivate: (id: string) => void;
  /** Host workspace: closable tabs. Omit for static mode (e.g. DbExplorer). */
  onClose?: (id: string) => void;
  onPin?: (id: string) => void;
  onDuplicate?: (id: string) => void;
  onReorder?: (fromIndex: number, toIndex: number) => void;
  extraLeading?: React.ReactNode;
};

function kindIcon(kind: string) {
  if (kind === "table_data") return <Table2 className="h-3 w-3 shrink-0 opacity-70" />;
  if (kind === "table_schema") return <Table2 className="h-3 w-3 shrink-0 text-violet-300/80" />;
  if (kind === "sql") return <FileCode2 className="h-3 w-3 shrink-0 text-amber-300/80" />;
  return <Table2 className="h-3 w-3 shrink-0 opacity-50" />;
}

export function DbTabsBar({
  tabs,
  activeId,
  onActivate,
  onClose,
  onPin,
  onDuplicate,
  onReorder,
  extraLeading,
}: Props) {
  const dragFrom = useRef<number | null>(null);

  const onDragStart = useCallback((index: number) => {
    dragFrom.current = index;
  }, []);

  const onDragOver = useCallback(
    (e: React.DragEvent, index: number) => {
      e.preventDefault();
    },
    [],
  );

  const onDrop = useCallback(
    (toIndex: number) => {
      const from = dragFrom.current;
      dragFrom.current = null;
      if (from == null || from === toIndex || !onReorder) return;
      onReorder(from, toIndex);
    },
    [onReorder],
  );

  return (
    <div className="flex min-h-0 shrink-0 items-center gap-1 border-b border-red-900/25 bg-black/20 px-1 py-0.5">
      {extraLeading}
      <div className="flex min-w-0 flex-1 items-stretch gap-0.5 overflow-x-auto scrollbar-thin">
        {tabs.map((tab, index) => {
          const active = tab.id === activeId;
          return (
            <div
              key={tab.id}
              draggable={Boolean(onReorder)}
              onDragStart={() => onDragStart(index)}
              onDragOver={(e) => onDragOver(e, index)}
              onDrop={() => onDrop(index)}
              className={`group flex max-w-[11rem] shrink-0 items-center gap-0.5 rounded-t border border-transparent px-1.5 py-1 text-[10px] ${
                active
                  ? "border-b-2 border-b-red-500 bg-red-950/35 text-amber-100"
                  : "text-slate-500 hover:bg-white/5 hover:text-slate-300"
              }`}
            >
              <button
                type="button"
                className="flex min-w-0 flex-1 items-center gap-1 text-left"
                onClick={() => onActivate(tab.id)}
                onAuxClick={(e) => {
                  if (e.button === 1 && onDuplicate) {
                    e.preventDefault();
                    onDuplicate(tab.id);
                  }
                }}
              >
                {kindIcon(tab.kind)}
                {tab.pinned ? <Pin className="h-2.5 w-2.5 shrink-0 text-amber-400/90" /> : null}
                <span className="truncate font-medium">{tab.title}</span>
              </button>
              {onPin ? (
                <button
                  type="button"
                  title="Pin"
                  className="rounded p-0.5 opacity-0 hover:bg-white/10 group-hover:opacity-100"
                  onClick={(e) => {
                    e.stopPropagation();
                    onPin(tab.id);
                  }}
                >
                  <Pin className="h-3 w-3" />
                </button>
              ) : null}
              {onClose ? (
                <button
                  type="button"
                  title="Close"
                  className="rounded p-0.5 opacity-0 hover:bg-rose-950/50 group-hover:opacity-100"
                  onClick={(e) => {
                    e.stopPropagation();
                    onClose(tab.id);
                  }}
                >
                  <X className="h-3 w-3" />
                </button>
              ) : null}
            </div>
          );
        })}
      </div>
    </div>
  );
}
