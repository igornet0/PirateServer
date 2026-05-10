import { X } from "lucide-react";
import React, { useEffect, useRef } from "react";

type Props = {
  open: boolean;
  title: string;
  onClose: () => void;
  children: React.ReactNode;
  widthClassName?: string;
};

export function DbDrawer({ open, title, onClose, children, widthClassName = "max-w-lg" }: Props) {
  const panelRef = useRef<HTMLDivElement>(null);

  useEffect(() => {
    if (!open) return;
    const onKey = (e: KeyboardEvent) => {
      if (e.key === "Escape") onClose();
    };
    window.addEventListener("keydown", onKey);
    return () => window.removeEventListener("keydown", onKey);
  }, [open, onClose]);

  if (!open) return null;

  return (
    <div className="pointer-events-auto absolute inset-0 z-20 flex justify-end">
      <button
        type="button"
        className="absolute inset-0 bg-black/50"
        aria-label="Close drawer"
        onClick={onClose}
      />
      <div
        ref={panelRef}
        className={`relative flex h-full w-full ${widthClassName} animate-in slide-in-from-right border-l border-red-900/35 bg-slate-950/98 shadow-2xl duration-200`}
      >
        <div className="flex min-h-0 min-w-0 flex-1 flex-col">
          <div className="flex shrink-0 items-center justify-between border-b border-red-900/25 px-3 py-2">
            <h3 className="text-xs font-semibold text-slate-100">{title}</h3>
            <button
              type="button"
              onClick={onClose}
              className="rounded p-1 text-slate-400 hover:bg-white/10 hover:text-slate-100"
            >
              <X className="h-4 w-4" />
            </button>
          </div>
          <div className="min-h-0 flex-1 overflow-auto p-3 text-[11px] text-slate-200">{children}</div>
        </div>
      </div>
    </div>
  );
}
