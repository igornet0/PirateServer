/**
 * Tauri WebView uses an app origin (e.g. https://tauri.localhost), so `fetch()` to the
 * user’s Control API is cross-origin and blocked by CORS unless the server allows it.
 * `@tauri-apps/plugin-http` runs requests from the Rust side and bypasses CORS.
 *
 * Web / nginx same-origin builds keep using the global `fetch`.
 */
export function deployFetch(input: RequestInfo | URL, init?: RequestInit): Promise<Response> {
  if (import.meta.env.VITE_DEPLOY_DESKTOP === "1") {
    return import("@tauri-apps/plugin-http").then(({ fetch }) => fetch(input, init));
  }
  return fetch(input, init);
}
