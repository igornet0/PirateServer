import React, { useMemo, useState } from "react";
import { ChevronsDown, ChevronsUp, FileText, LayoutList } from "lucide-react";
import { useI18n } from "../i18n";
import {
  boolToEnv,
  envTruthy,
  parseDotEnv,
  serializeDotEnv,
} from "./parseSerialize";
import {
  SERVER_DEPLOY_ENV_KNOWN_KEYS,
  SERVER_DEPLOY_ENV_SCHEMA,
  type ServerEnvVarDef,
} from "./schema";

/** По умолчанию открыта только первая группа — длинная форма короче в высоту. */
function initialCategoryCollapsed(): Record<string, boolean> {
  const o: Record<string, boolean> = {};
  SERVER_DEPLOY_ENV_SCHEMA.forEach((c, i) => {
    if (i > 0) o[c.id] = true;
  });
  return o;
}

const fieldClass =
  "w-full rounded-lg border border-white/10 bg-black/30 px-3 py-2 font-mono text-xs text-slate-100 placeholder:text-slate-600 focus:border-amber-600/45 focus:outline-none";
const labelClass = "mb-0.5 block text-xs font-medium text-slate-400";
const hintClass = "mt-0.5 text-[11px] leading-snug text-slate-500";

/** ~48 байт энтропии в base64 (аналог openssl rand -base64 48). */
function generateRandomJwtSecret(): string {
  const bytes = new Uint8Array(48);
  crypto.getRandomValues(bytes);
  let bin = "";
  for (let i = 0; i < bytes.length; i++) {
    bin += String.fromCharCode(bytes[i]!);
  }
  return btoa(bin);
}

function FieldInput({
  def,
  raw,
  disabled,
  onChange,
  tr,
}: {
  def: ServerEnvVarDef;
  raw: string | undefined;
  disabled?: boolean;
  onChange: (v: string) => void;
  tr: (ru: string, en: string) => string;
}) {
  if (def.type === "boolean") {
    const on = envTruthy(raw);
    return (
      <label className="flex cursor-pointer items-center gap-2 text-sm text-slate-200">
        <input
          type="checkbox"
          disabled={disabled}
          checked={on}
          onChange={(e) => onChange(boolToEnv(e.target.checked))}
          className="rounded border-white/20 bg-black/40 text-amber-600 focus:ring-amber-600/60"
        />
        <span className="text-xs">{on ? tr("вкл (1)", "on (1)") : tr("выкл (0)", "off (0)")}</span>
      </label>
    );
  }
  const type = def.type === "password" ? "password" : "text";
  const rows = def.type === "textarea" ? 3 : undefined;
  if (def.type === "password" && def.key === "CONTROL_API_JWT_SECRET") {
    return (
      <div className="flex flex-wrap items-stretch gap-2">
        <input
          type="password"
          disabled={disabled}
          value={raw ?? ""}
          onChange={(e) => onChange(e.target.value)}
          className={`${fieldClass} min-w-0 flex-1`}
          spellCheck={false}
          autoComplete="new-password"
        />
        <button
          type="button"
          disabled={disabled}
          onClick={() => onChange(generateRandomJwtSecret())}
          className="shrink-0 rounded-lg border border-amber-800/50 bg-amber-950/40 px-3 py-2 text-xs font-semibold text-amber-100/95 transition hover:bg-amber-950/60 disabled:opacity-40"
        >
          {tr("Сгенерировать", "Generate")}
        </button>
      </div>
    );
  }
  if (rows) {
    return (
      <textarea
        disabled={disabled}
        value={raw ?? ""}
        onChange={(e) => onChange(e.target.value)}
        rows={rows}
        className={fieldClass}
        spellCheck={false}
        autoComplete="off"
      />
    );
  }
  return (
    <input
      type={type}
      disabled={disabled}
      value={raw ?? ""}
      onChange={(e) => onChange(e.target.value)}
      className={fieldClass}
      spellCheck={false}
      autoComplete={def.type === "password" ? "new-password" : "off"}
    />
  );
}

export function HostServerEnvPanel({
  value,
  onChange,
  disabled,
  hiddenKeys,
}: {
  value: string;
  onChange: (next: string) => void;
  disabled?: boolean;
  hiddenKeys?: ReadonlySet<string>;
}) {
  const { language, t } = useI18n();
  const tr = (ru: string, en: string) => (language === "ru" ? ru : en);
  const [viewMode, setViewMode] = useState<"form" | "raw">("form");
  const [extraKeyDraft, setExtraKeyDraft] = useState("");
  const [extraValDraft, setExtraValDraft] = useState("");
  const [collapsed, setCollapsed] = useState<Record<string, boolean>>(initialCategoryCollapsed);

  const map = useMemo(() => parseDotEnv(value), [value]);

  const setKey = (key: string, val: string) => {
    const next = new Map(map);
    const t = val.trim();
    if (t === "") next.delete(key);
    else next.set(key, val);
    onChange(serializeDotEnv(next));
  };

  const unknownKeys = useMemo(
    () =>
      [...map.keys()]
        .filter((k) => !SERVER_DEPLOY_ENV_KNOWN_KEYS.has(k))
        .filter((k) => !(hiddenKeys?.has(k) ?? false))
        .sort(),
    [map, hiddenKeys],
  );

  const toggleCategory = (id: string) => {
    setCollapsed((c) => ({ ...c, [id]: !c[id] }));
  };

  const addExtra = () => {
    const k = extraKeyDraft.trim();
    if (!k || !/^[A-Za-z_][A-Za-z0-9_]*$/.test(k)) return;
    setKey(k, extraValDraft);
    setExtraKeyDraft("");
    setExtraValDraft("");
  };

  return (
    <div className="space-y-3">
      <div className="flex flex-wrap items-center gap-2">
        <span className="text-xs text-slate-500">{t("auto.serverDeployEnv_HostServerEnvPanel_tsx.4")}</span>
        <button
          type="button"
          disabled={disabled}
          onClick={() => setViewMode("form")}
          className={`inline-flex items-center gap-1.5 rounded-lg px-3 py-1.5 text-xs font-medium ${
            viewMode === "form"
              ? "bg-amber-900/45 text-amber-100 ring-1 ring-amber-600/45"
              : "bg-white/5 text-slate-400 hover:bg-white/10"
          }`}
        >
          <LayoutList className="h-3.5 w-3.5" />
          {t("auto.serverDeployEnv_HostServerEnvPanel_tsx.5")}
        </button>
        <button
          type="button"
          disabled={disabled}
          onClick={() => setViewMode("raw")}
          className={`inline-flex items-center gap-1.5 rounded-lg px-3 py-1.5 text-xs font-medium ${
            viewMode === "raw"
              ? "bg-amber-900/45 text-amber-100 ring-1 ring-amber-600/45"
              : "bg-white/5 text-slate-400 hover:bg-white/10"
          }`}
        >
          <FileText className="h-3.5 w-3.5" />
          {t("auto.serverDeployEnv_HostServerEnvPanel_tsx.6")}
        </button>
      </div>

      {viewMode === "raw" ? (
        <textarea
          disabled={disabled}
          value={value}
          onChange={(e) => onChange(e.target.value)}
          rows={16}
          className="w-full rounded-xl border border-white/10 bg-black/35 px-3 py-2 font-mono text-xs text-slate-100 focus:border-amber-600/45 focus:outline-none"
          spellCheck={false}
        />
      ) : (
        <div className="space-y-2">
          {SERVER_DEPLOY_ENV_SCHEMA.map((cat) => {
            const isOpen = !collapsed[cat.id];
            const visibleVars = cat.vars.filter((v) => !(hiddenKeys?.has(v.key) ?? false));
            if (visibleVars.length === 0) return null;
            return (
              <div
                key={cat.id}
                className="overflow-hidden rounded-xl border border-white/10 bg-black/20"
              >
                <button
                  type="button"
                  onClick={() => toggleCategory(cat.id)}
                  className="flex w-full items-center justify-between gap-2 px-3 py-2 text-left text-sm font-semibold text-slate-200 hover:bg-white/5"
                >
                  {cat.title}
                  {isOpen ? (
                    <ChevronsUp className="h-4 w-4 shrink-0 text-slate-500" />
                  ) : (
                    <ChevronsDown className="h-4 w-4 shrink-0 text-slate-500" />
                  )}
                </button>
                {isOpen ? (
                  <div className="space-y-4 border-t border-white/5 px-3 py-3">
                    {visibleVars.map((def) => (
                      <div key={def.key}>
                        <label className={labelClass} htmlFor={`env-${def.key}`}>
                          <code className="text-amber-200/90">{def.key}</code>
                          <span className="ml-2 font-sans text-slate-400">{def.label}</span>
                        </label>
                        {def.hint ? <p className={hintClass}>{def.hint}</p> : null}
                        <div id={`env-${def.key}`}>
                          <FieldInput
                            def={def}
                            raw={map.get(def.key)}
                            disabled={disabled}
                            onChange={(v) => setKey(def.key, v)}
                            tr={tr}
                          />
                        </div>
                      </div>
                    ))}
                  </div>
                ) : null}
              </div>
            );
          })}

          <div className="rounded-xl border border-dashed border-white/15 bg-black/15 p-3">
            <p className="mb-2 text-xs font-medium text-slate-400">
              {t("auto.serverDeployEnv_HostServerEnvPanel_tsx.7")}
              {unknownKeys.length ? (
                <span className="ml-2 text-slate-500">({unknownKeys.length})</span>
              ) : null}
            </p>
            {unknownKeys.length ? (
              <div className="mb-3 space-y-2">
                {unknownKeys.map((key) => (
                  <div key={key} className="flex flex-wrap items-start gap-2">
                    <code className="mt-2 shrink-0 text-[11px] text-emerald-200/85">{key}</code>
                    <input
                      disabled={disabled}
                      value={map.get(key) ?? ""}
                      onChange={(e) => setKey(key, e.target.value)}
                      className={`${fieldClass} min-w-[12rem] flex-1`}
                      spellCheck={false}
                    />
                    <button
                      type="button"
                      disabled={disabled}
                      onClick={() => {
                        const next = new Map(map);
                        next.delete(key);
                        onChange(serializeDotEnv(next));
                      }}
                      className="mt-1 rounded-lg px-2 py-1 text-xs text-rose-300 hover:bg-rose-950/40"
                    >
                      {t("auto.serverDeployEnv_HostServerEnvPanel_tsx.8")}
                    </button>
                  </div>
                ))}
              </div>
            ) : (
              <p className="mb-2 text-[11px] text-slate-500">{t("auto.serverDeployEnv_HostServerEnvPanel_tsx.9")}</p>
            )}
            <div className="flex flex-wrap items-end gap-2">
              <div className="min-w-[8rem] flex-1">
                <label className={labelClass}>{t("auto.serverDeployEnv_HostServerEnvPanel_tsx.10")}</label>
                <input
                  disabled={disabled}
                  value={extraKeyDraft}
                  onChange={(e) => setExtraKeyDraft(e.target.value)}
                  placeholder="MY_VAR"
                  className={fieldClass}
                  spellCheck={false}
                />
              </div>
              <div className="min-w-[10rem] flex-[2]">
                <label className={labelClass}>{t("auto.serverDeployEnv_HostServerEnvPanel_tsx.11")}</label>
                <input
                  disabled={disabled}
                  value={extraValDraft}
                  onChange={(e) => setExtraValDraft(e.target.value)}
                  className={fieldClass}
                  spellCheck={false}
                />
              </div>
              <button
                type="button"
                disabled={disabled || !extraKeyDraft.trim()}
                onClick={() => addExtra()}
                className="mb-0.5 rounded-lg border border-white/15 bg-white/5 px-3 py-2 text-xs text-slate-200 hover:bg-white/10 disabled:opacity-40"
              >
                {t("auto.serverDeployEnv_HostServerEnvPanel_tsx.12")}
              </button>
            </div>
          </div>
        </div>
      )}
    </div>
  );
}
