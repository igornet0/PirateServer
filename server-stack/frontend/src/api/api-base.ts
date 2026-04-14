/**
 * Optional absolute base for control-api (Tauri / cross-origin).
 * Web + nginx: unset → same-origin relative `/api/...`.
 *
 * Desktop (Tauri): user may override via localStorage (`deploy.controlApiBase`).
 */

export const CONTROL_API_BASE_STORAGE_KEY = "deploy.controlApiBase";
export const DESKTOP_SETUP_DONE_KEY = "deploy.desktopSetupDone";
/** Optional FQDN from desktop setup (same role as `pirate_DOMAIN` in install.sh). */
export const DESKTOP_SETUP_DOMAIN_KEY = "deploy.desktopSetupDomain";
/** Prefill for web dashboard login — same as CONTROL_UI_ADMIN_USERNAME after install.sh --ui. */
export const DESKTOP_PREFILL_USERNAME_KEY = "deploy.desktopPrefillUsername";
/**
 * User expectation: server has PIRATE_DISPLAY_STREAM_CONSENT=1 (set on host during install.sh).
 * Does not change the server; used for local hints only.
 */
export const DESKTOP_DISPLAY_STREAM_INTENT_KEY = "deploy.desktopDisplayStreamIntent";

function trimTrailingSlash(s: string): string {
  return s.replace(/\/$/, "");
}

/** True when built from server-stack/desktop-ui (Tauri shell). */
export function isDeployDesktopApp(): boolean {
  return import.meta.env.VITE_DEPLOY_DESKTOP === "1";
}

export type ControlApiBaseValidationError = "empty" | "invalid" | "scheme";

export function parseAndValidateControlApiBase(
  input: string,
):
  | { ok: true; base: string }
  | { ok: false; error: ControlApiBaseValidationError } {
  const trimmed = input.trim();
  if (!trimmed) {
    return { ok: false, error: "empty" };
  }
  let u: URL;
  try {
    u = new URL(trimmed);
  } catch {
    return { ok: false, error: "invalid" };
  }
  if (u.protocol !== "http:" && u.protocol !== "https:") {
    return { ok: false, error: "scheme" };
  }
  const path = u.pathname.replace(/\/$/, "");
  const base = trimTrailingSlash(`${u.origin}${path}`);
  return { ok: true, base };
}

function readStoredBase(): string | null {
  try {
    const raw = localStorage.getItem(CONTROL_API_BASE_STORAGE_KEY)?.trim();
    if (!raw) {
      return null;
    }
    const v = parseAndValidateControlApiBase(raw);
    if (!v.ok) {
      return null;
    }
    return v.base;
  } catch {
    return null;
  }
}

/** Persisted Control API base (after setup), or null. */
export function getStoredControlApiBase(): string | null {
  return readStoredBase();
}

export function setControlApiBaseOverride(base: string): void {
  const v = parseAndValidateControlApiBase(base);
  if (!v.ok) {
    throw new Error(`invalid control API base: ${v.error}`);
  }
  try {
    localStorage.setItem(CONTROL_API_BASE_STORAGE_KEY, v.base);
  } catch {
    /* ignore */
  }
}

export function clearControlApiBaseOverride(): void {
  try {
    localStorage.removeItem(CONTROL_API_BASE_STORAGE_KEY);
  } catch {
    /* ignore */
  }
}

export function markDesktopSetupDone(): void {
  try {
    localStorage.setItem(DESKTOP_SETUP_DONE_KEY, "1");
  } catch {
    /* ignore */
  }
}

export function clearDesktopSetupDone(): void {
  try {
    localStorage.removeItem(DESKTOP_SETUP_DONE_KEY);
  } catch {
    /* ignore */
  }
}

export function needsDesktopFirstRunSetup(): boolean {
  if (!isDeployDesktopApp()) {
    return false;
  }
  try {
    return localStorage.getItem(DESKTOP_SETUP_DONE_KEY) !== "1";
  } catch {
    return true;
  }
}

/** Default URL shown in the desktop setup field when nothing is stored yet. */
export function defaultControlApiBaseForDesktop(): string {
  const raw = import.meta.env.VITE_CONTROL_API_BASE;
  if (typeof raw === "string" && raw.trim()) {
    const v = parseAndValidateControlApiBase(raw);
    if (v.ok) {
      return v.base;
    }
  }
  return "http://127.0.0.1:8080";
}

export function controlApiBaseForDesktopSetupField(): string {
  return getStoredControlApiBase() ?? defaultControlApiBaseForDesktop();
}

export function getDesktopSetupDomain(): string {
  try {
    return localStorage.getItem(DESKTOP_SETUP_DOMAIN_KEY)?.trim() ?? "";
  } catch {
    return "";
  }
}

export function setDesktopSetupDomain(domain: string): void {
  try {
    const t = domain.trim();
    if (!t) {
      localStorage.removeItem(DESKTOP_SETUP_DOMAIN_KEY);
    } else {
      localStorage.setItem(DESKTOP_SETUP_DOMAIN_KEY, t);
    }
  } catch {
    /* ignore */
  }
}

/** Default matches install.sh web dashboard default. */
export function defaultDesktopPrefillUsername(): string {
  return "admin";
}

export function getDesktopPrefillUsername(): string {
  try {
    const s = localStorage.getItem(DESKTOP_PREFILL_USERNAME_KEY)?.trim();
    if (s) {
      return s;
    }
  } catch {
    /* ignore */
  }
  return defaultDesktopPrefillUsername();
}

export function setDesktopPrefillUsername(username: string): void {
  try {
    const t = username.trim();
    if (!t) {
      localStorage.removeItem(DESKTOP_PREFILL_USERNAME_KEY);
    } else {
      localStorage.setItem(DESKTOP_PREFILL_USERNAME_KEY, t);
    }
  } catch {
    /* ignore */
  }
}

export function getDesktopDisplayStreamIntent(): boolean {
  try {
    return localStorage.getItem(DESKTOP_DISPLAY_STREAM_INTENT_KEY) === "1";
  } catch {
    return false;
  }
}

export function setDesktopDisplayStreamIntent(on: boolean): void {
  try {
    if (on) {
      localStorage.setItem(DESKTOP_DISPLAY_STREAM_INTENT_KEY, "1");
    } else {
      localStorage.removeItem(DESKTOP_DISPLAY_STREAM_INTENT_KEY);
    }
  } catch {
    /* ignore */
  }
}

export function apiBase(): string {
  const stored = readStoredBase();
  if (stored) {
    return stored;
  }
  const raw = import.meta.env.VITE_CONTROL_API_BASE;
  if (typeof raw === "string" && raw.trim()) {
    return trimTrailingSlash(raw.trim());
  }
  return "";
}

/** Prefix `path` (must start with `/`) with absolute base when set. */
export function apiUrl(path: string): string {
  const b = apiBase();
  if (!b) {
    return path;
  }
  if (!path.startsWith("/")) {
    return `${b}/${path}`;
  }
  return `${b}${path}`;
}
