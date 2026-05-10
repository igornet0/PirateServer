import { Eye, EyeOff, RefreshCw } from "lucide-react";
import React, { useState } from "react";

/** 32 bytes hex — достаточно для master key / паролей install & runtime. */
export function generateHostServiceSecretToken(): string {
  const bytes = new Uint8Array(32);
  crypto.getRandomValues(bytes);
  return Array.from(bytes, (b) => b.toString(16).padStart(2, "0")).join("");
}

/** Ключи env на хосте, для которых в runtime-редакторе показываем «сгенерировать» и показ/скрытие. */
export function isSensitiveHostEnvKey(key: string): boolean {
  const k = key.trim();
  if (!k) return false;
  const u = k.toUpperCase();
  const explicit = new Set(["MEILI_MASTER_KEY", "MINIO_ROOT_PASSWORD", "PIRATE_EXPLORER_DB_PASSWORD"]);
  if (explicit.has(u)) return true;
  if (/_PASSWORD$/i.test(k)) return true;
  if (/_TOKEN$/i.test(k)) return true;
  if (/_SECRET$/i.test(k) || /_SECRET_/i.test(k)) return true;
  if (/_API_KEY$/i.test(k)) return true;
  if (/_ACCESS_KEY$/i.test(k)) return true;
  if (/_MASTER_KEY$/i.test(k)) return true;
  return false;
}

const btnIcon =
  "inline-flex shrink-0 items-center justify-center rounded-lg border border-white/10 bg-white/5 px-2.5 py-2 text-slate-400 transition hover:bg-white/10 hover:text-slate-200 focus-visible:outline-none focus-visible:ring-2 focus-visible:ring-amber-600/60 disabled:opacity-40";

const btnGenerate =
  "inline-flex shrink-0 items-center justify-center gap-1 rounded-lg border border-amber-800/50 bg-amber-950/40 px-2.5 py-2 text-[11px] font-semibold text-amber-100/95 transition hover:bg-amber-950/60 focus-visible:outline-none focus-visible:ring-2 focus-visible:ring-amber-600/60 disabled:opacity-40";

type SecretFieldProps = {
  value: string;
  onChange: (v: string) => void;
  tr: (ru: string, en: string) => string;
  /** Tailwind for the text input (width, font). */
  inputClassName: string;
  placeholder?: string;
};

/**
 * Поле секрета: показ/скрытие и кнопка «Сгенерировать» (криптостойкий hex).
 */
export function SecretFieldRow({ value, onChange, tr, inputClassName, placeholder }: SecretFieldProps) {
  const [visible, setVisible] = useState(false);

  return (
    <div className="flex w-full min-w-0 items-stretch gap-1.5">
      <input
        className={inputClassName}
        type={visible ? "text" : "password"}
        autoComplete="off"
        spellCheck={false}
        value={value}
        onChange={(e) => onChange(e.target.value)}
        placeholder={placeholder}
      />
      <button
        type="button"
        className={btnIcon}
        onClick={() => setVisible((v) => !v)}
        title={visible ? tr("Скрыть", "Hide") : tr("Показать", "Show")}
        aria-label={visible ? tr("Скрыть значение", "Hide value") : tr("Показать значение", "Show value")}
        aria-pressed={visible}
      >
        {visible ? <EyeOff className="h-4 w-4" /> : <Eye className="h-4 w-4" />}
      </button>
      <button
        type="button"
        className={btnGenerate}
        onClick={() => onChange(generateHostServiceSecretToken())}
        title={tr("Сгенерировать случайное значение", "Generate random value")}
        aria-label={tr("Сгенерировать", "Generate")}
      >
        <RefreshCw className="h-3.5 w-3.5" />
        <span className="hidden sm:inline">{tr("Сгенерировать", "Generate")}</span>
      </button>
    </div>
  );
}
