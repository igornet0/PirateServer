/** Matches Rust `pirate_desktop::RegisteredProject` (serde camelCase). */
export type RegisteredProject = {
  name: string;
  path: string;
  localVersion: string;
  deployProjectId: string;
  serverProjectVersion: string;
  connected: boolean;
  needsDeploy: boolean;
};
