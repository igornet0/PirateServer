import { Activity, AlertCircle, Link2 } from "lucide-react";
import React from "react";
import { useI18n } from "./i18n";

/** Right column when the main area is «Обзор», «Соединение» or «Интернет» (not «Проекты»). */
export function OverviewContextPanel({
  endpoint,
  grpcErr,
  processBadgeClass,
  processDotClass,
  processStateLabel,
  deployedLabel,
  deployedValue,
  projectVersion,
  tab,
  onOpenConnection,
}: {
  endpoint: string | null;
  grpcErr: string | null;
  processBadgeClass: string;
  processDotClass: string;
  processStateLabel: string;
  deployedLabel: string | null;
  deployedValue: string;
  projectVersion: string | null;
  tab: "overview" | "connection" | "internet";
  onOpenConnection: () => void;
}) {
  const { language, t } = useI18n();
  const tr = (ru: string, en: string) => (language === "ru" ? ru : en);
  return (
    <aside className="flex w-full shrink-0 flex-col border-t border-border-subtle bg-panel lg:w-[300px] lg:border-l lg:border-t-0">
      <div className="border-b border-border-subtle px-3 py-3">
        <p className="text-[10px] font-semibold uppercase tracking-wider text-red-400/70">{t("context.connection")}</p>
        {endpoint ? (
          <p className="mt-2 text-[11px] leading-relaxed text-slate-500">
            {tr(
              "Адрес gRPC и статус — в основной колонке и в шапке (здесь без дублирования).",
              "gRPC address and status are in the main column and header (not duplicated here).",
            )}
          </p>
        ) : (
          <p className="mt-2 text-xs text-slate-500">{t("context.connectPrompt")}</p>
        )}
        {grpcErr ? (
          <p className="mt-2 flex items-start gap-1.5 text-xs text-rose-300">
            <AlertCircle className="mt-0.5 h-3.5 w-3.5 shrink-0" />
            {grpcErr}
          </p>
        ) : null}
        {endpoint ? (
          <div className="mt-3 space-y-2 text-[11px] text-slate-400">
            <div className={`inline-flex items-center gap-2 rounded-md px-2 py-1 text-xs font-medium ${processBadgeClass}`}>
              <span className={`h-2 w-2 rounded-full ${processDotClass}`} />
              {t("context.process")}: {processStateLabel}
            </div>
            {deployedLabel ? (
              <p>
                {deployedLabel}{" "}
                <span className="font-mono text-slate-200">{deployedValue}</span>
              </p>
            ) : (
              <p>
                {t("context.version")}: <span className="font-mono text-slate-200">{deployedValue || "—"}</span>
              </p>
            )}
            {projectVersion?.trim() ? (
              <p>
                pirate.toml: <span className="font-mono text-slate-200">{projectVersion.trim()}</span>
              </p>
            ) : null}
          </div>
        ) : null}
        <button
          type="button"
          onClick={onOpenConnection}
          className="mt-3 w-full rounded-lg border border-red-900/40 bg-red-950/35 px-3 py-2 text-xs font-medium text-orange-100 hover:bg-red-950/55 hover:shadow-glow"
        >
          <Link2 className="mr-1.5 inline h-3.5 w-3.5" />
          {t("context.connectionSettings")}
        </button>
      </div>
      <div className="px-3 py-3 text-[11px] leading-relaxed text-slate-500">
        {tab === "overview" ? (
          <p>
            <Activity className="mr-1 inline h-3.5 w-3.5 text-red-500/60" />
            {t("context.overviewTip")}
          </p>
        ) : tab === "connection" ? (
          <p>{t("context.connectionTip")}</p>
        ) : (
          <p>{t("context.internetTip")}</p>
        )}
      </div>
    </aside>
  );
}
