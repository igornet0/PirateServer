import { invoke } from "@tauri-apps/api/core";

export type CmdPlaceholder = {
  name: string;
  options?: string[];
};

export async function fetchCmdPlaceholders(path: string, phases: string[]): Promise<CmdPlaceholder[]> {
  const raw = await invoke<string>("project_cmd_placeholders", { path, phases });
  return JSON.parse(raw) as CmdPlaceholder[];
}

export function defaultCmdVarValues(placeholders: CmdPlaceholder[]): Record<string, string> {
  const out: Record<string, string> = {};
  for (const p of placeholders) {
    out[p.name] = p.options?.[0] ?? "";
  }
  return out;
}

/** `null` when empty map — Tauri optional arg. */
export function cmdVarsInvokeArg(values: Record<string, string>): Record<string, string> | null {
  const trimmed: Record<string, string> = {};
  for (const [k, v] of Object.entries(values)) {
    const t = v.trim();
    if (t) trimmed[k] = t;
  }
  return Object.keys(trimmed).length > 0 ? trimmed : null;
}
