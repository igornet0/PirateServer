/** Serialized `pirate_desktop::ToolchainReport` from `probe_local_toolchain`. */

export type ToolchainItem = {
  id: string;
  label: string;
  installed: boolean;
  /** Distinct version lines (several interpreters / installs possible). */
  versions: string[];
  installHint: string;
};

export type ToolchainReport = {
  items: ToolchainItem[];
  generatedAtMs: number;
};
