/// <reference types="vite/client" />

interface ImportMetaEnv {
  readonly VITE_APP_RELEASE: string;
}

interface ImportMeta {
  readonly env: ImportMetaEnv;
}
