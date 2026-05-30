/** Matches Rust `pirate_desktop::RegisteredProject` (serde camelCase). */
export type RegisteredProject = {
  name: string;
  path: string;
  localVersion: string;
  deployProjectId: string;
  serverProjectVersion: string;
  connected: boolean;
  needsDeploy: boolean;
  /** Unix ms — last desktop deploy recorded for this folder */
  lastDeployAtMs?: number | null;
  /** Server-reported deployed version after last desktop deploy */
  lastDeployedVersion?: string | null;
};
