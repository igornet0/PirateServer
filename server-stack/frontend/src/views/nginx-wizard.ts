/**
 * Build a minimal `server { }` fragment for listen + optional TLS + static + `/api/` proxy.
 * Operators paste into the full nginx config or merge with existing `http` context.
 */

import { t } from "../i18n/index.js";

export function buildNginxServerSnippet(opts: {
  listen: string;
  serverName: string;
  rootPath: string;
  apiUpstream: string;
  certPath: string;
  keyPath: string;
  tls: boolean;
}): string {
  const tls = opts.tls
    ? `
    ssl_certificate     ${opts.certPath};
    ssl_certificate_key ${opts.keyPath};`
    : "";

  return `
server {
    listen ${opts.listen}${opts.tls ? " ssl" : ""};
    server_name ${opts.serverName};
${tls}
    root ${opts.rootPath};
    index index.html;

    location = /login {
        try_files /login.html =404;
    }

    location /api/ {
        proxy_pass http://${opts.apiUpstream};
        proxy_http_version 1.1;
        proxy_set_header Host $host;
        proxy_set_header X-Real-IP $remote_addr;
        proxy_set_header X-Forwarded-For $proxy_add_x_forwarded_for;
        proxy_set_header X-Forwarded-Proto $scheme;
    }

    location / {
        try_files $uri $uri/ /index.html;
    }
}
`.trimStart();
}

export function bindNginxWizard(): void {
  const btn = document.getElementById("nginx-wizard-insert");
  if (!btn) {
    return;
  }
  btn.addEventListener("click", () => {
    const listen = (
      document.getElementById("wiz-listen") as HTMLInputElement
    ).value.trim() || "[::]:80";
    const serverName = (
      document.getElementById("wiz-server-name") as HTMLInputElement
    ).value.trim() || "_";
    const rootPath = (
      document.getElementById("wiz-root") as HTMLInputElement
    ).value.trim() || "/var/www/deploy-dashboard";
    let apiUpstream = (
      document.getElementById("wiz-api-upstream") as HTMLInputElement
    ).value.trim();
    apiUpstream = apiUpstream.replace(/^https?:\/\//i, "");
    if (!apiUpstream) {
      apiUpstream = "127.0.0.1:8080";
    }
    const certPath = (
      document.getElementById("wiz-cert") as HTMLInputElement
    ).value.trim();
    const keyPath = (
      document.getElementById("wiz-key") as HTMLInputElement
    ).value.trim();
    const tls = (document.getElementById("wiz-tls") as HTMLInputElement)
      ?.checked;

    const editor = document.getElementById("nginx-editor") as HTMLTextAreaElement;
    const snippet = buildNginxServerSnippet({
      listen,
      serverName,
      rootPath,
      apiUpstream,
      certPath: certPath || "/etc/ssl/certs/server.pem",
      keyPath: keyPath || "/etc/ssl/private/server.key",
      tls: Boolean(tls),
    });
    editor.value = `${editor.value.trimEnd()}\n\n${snippet}\n`;
    const result = document.getElementById("nginx-result")!;
    result.textContent = t("nginx.snippetDone");
  });
}
