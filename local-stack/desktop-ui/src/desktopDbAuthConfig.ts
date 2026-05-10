/**
 * Set `VITE_DESKTOP_DB_AUTH_REQUIRED=0` in `.env` to fall back to server-side
 * DSN env only (no per-request X-Pirate-Db-User/Password) while rolling out the feature.
 * Default: require explicit browser credentials for host DB content APIs.
 */
export const desktopDbAuthRequired =
  typeof import.meta !== "undefined" &&
  import.meta.env &&
  (import.meta.env as { VITE_DESKTOP_DB_AUTH_REQUIRED?: string }).VITE_DESKTOP_DB_AUTH_REQUIRED !==
    "0";
