/** Matches `ProjectsPreflightReport` from desktop `preflight.rs`. */
export type PreflightItem = {
  id: string;
  ok: boolean;
  title: string;
  detail: string;
  hint?: string;
};

/** Matches `HostServicesCompatSummary` from desktop `host_services_compat.rs`. */
export type HostServicesCompatSummary = {
  status: string;
  skipReason?: string | null;
  requiredHostServiceIds: string[];
  missingHostServiceIds: string[];
  satisfiedHostServiceIds: string[];
  dispatchScriptPresent?: boolean | null;
};

export type ProjectsPreflightReport = {
  ready: boolean;
  checks: PreflightItem[];
  suggestedControlApi?: string | null;
  hostServicesCompat?: HostServicesCompatSummary | null;
};
