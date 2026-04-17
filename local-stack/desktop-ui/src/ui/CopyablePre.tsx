import { Check, Copy } from "lucide-react";
import React, { useState } from "react";
import { useI18n } from "../i18n";

const btnIcon =
  "inline-flex items-center justify-center rounded-lg border border-white/10 bg-white/5 p-1.5 text-slate-400 transition-colors duration-150 hover:bg-white/10 hover:text-slate-200 focus-visible:outline-none focus-visible:ring-2 focus-visible:ring-red-600/80";

type Props = {
  value: string | null | undefined;
  placeholder?: string;
  className?: string;
  /** Default max height before scroll */
  maxHeightClass?: string;
};

/** Monospace block with copy — for JSON / script output */
export function CopyablePre({
  value,
  placeholder = "—",
  className = "rounded-xl border border-white/10 bg-black/40 p-3 text-xs text-slate-200",
  maxHeightClass = "max-h-48",
}: Props) {
  const { language } = useI18n();
  const tr = (ru: string, en: string) => (language === "ru" ? ru : en);
  const [copied, setCopied] = useState(false);
  const text = value ?? "";

  const copy = () => {
    if (!text.trim()) return;
    void navigator.clipboard.writeText(text);
    setCopied(true);
    window.setTimeout(() => setCopied(false), 2000);
  };

  return (
    <div className="relative">
      {text.trim() ? (
        <div className="mb-1 flex justify-end">
          <button
            type="button"
            onClick={() => copy()}
            className={btnIcon}
            title={tr("Копировать", "Copy")}
            aria-label={tr("Копировать", "Copy")}
          >
            {copied ? <Check className="h-3.5 w-3.5 text-red-400" /> : <Copy className="h-3.5 w-3.5" />}
          </button>
        </div>
      ) : null}
      <pre
        className={`overflow-auto font-mono ${maxHeightClass} ${className}`}
      >
        {text.trim() ? text : placeholder}
      </pre>
    </div>
  );
}
