import type { CmdPlaceholder } from "./cmdVars";

type Props = {
  open: boolean;
  title: string;
  placeholders: CmdPlaceholder[];
  values: Record<string, string>;
  language: string;
  onChange: (name: string, value: string) => void;
  onConfirm: () => void;
  onCancel: () => void;
};

function tr(language: string, ru: string, en: string) {
  return language === "ru" ? ru : en;
}

export function CmdVarsModal({
  open,
  title,
  placeholders,
  values,
  language,
  onChange,
  onConfirm,
  onCancel,
}: Props) {
  if (!open) return null;

  return (
    <div
      className="fixed inset-0 z-[80] flex items-center justify-center bg-black/60 p-4"
      role="dialog"
      aria-modal="true"
    >
      <div className="w-full max-w-md rounded-xl border border-border-subtle bg-panel p-4 shadow-xl">
        <p className="text-sm font-semibold text-slate-100">{title}</p>
        <p className="mt-1 text-xs text-slate-400">
          {tr(
            language,
            "Параметры из pirate.toml (${NAME} или ${NAME=a|b}).",
            "Parameters from pirate.toml (${NAME} or ${NAME=a|b}).",
          )}
        </p>
        <div className="mt-4 space-y-3">
          {placeholders.map((p) => (
            <label key={p.name} className="block text-xs text-slate-300">
              <span className="font-mono text-orange-200/90">{p.name}</span>
              {p.options && p.options.length > 0 ? (
                <select
                  value={values[p.name] ?? ""}
                  onChange={(e) => onChange(p.name, e.target.value)}
                  className="mt-1 w-full rounded border border-border-subtle bg-black/30 px-2 py-1.5 text-xs text-slate-100"
                >
                  {p.options.map((opt) => (
                    <option key={opt} value={opt}>
                      {opt}
                    </option>
                  ))}
                </select>
              ) : (
                <input
                  value={values[p.name] ?? ""}
                  onChange={(e) => onChange(p.name, e.target.value)}
                  className="mt-1 w-full rounded border border-border-subtle bg-black/30 px-2 py-1.5 text-xs text-slate-100"
                  placeholder={tr(language, "значение", "value")}
                />
              )}
            </label>
          ))}
        </div>
        <div className="mt-4 flex justify-end gap-2">
          <button
            type="button"
            onClick={onCancel}
            className="rounded-lg border border-border-subtle px-3 py-1.5 text-xs text-slate-300 hover:bg-white/5"
          >
            {tr(language, "Отмена", "Cancel")}
          </button>
          <button
            type="button"
            onClick={onConfirm}
            className="rounded-lg bg-gradient-to-r from-red-800 to-red-700 px-3 py-1.5 text-xs font-semibold text-white"
          >
            {tr(language, "Запустить", "Run")}
          </button>
        </div>
      </div>
    </div>
  );
}
