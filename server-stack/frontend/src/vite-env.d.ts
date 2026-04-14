/// <reference types="vite/client" />

interface ImportMetaEnv {
  readonly VITE_CONTROL_API_BASE?: string;
  /** Set by server-stack/desktop-ui Vite config for Tauri builds. */
  readonly VITE_DEPLOY_DESKTOP?: string;
}

interface ImportMeta {
  readonly env: ImportMetaEnv;
}
