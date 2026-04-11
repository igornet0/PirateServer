import "./styles.css";
import { invoke } from "@tauri-apps/api/core";

type Status = {
  hostname: string;
  hosts_entry_ok: boolean;
  shell: string;
};

type GrpcConnectResult = {
  endpoint: string;
  currentVersion: string;
  state: string;
};

type DeployOutcome = {
  status: string;
  deployedVersion: string;
  artifactBytes: number;
  chunkCount: number;
};

type RollbackOutcome = {
  status: string;
  activeVersion: string;
};

type ServerBookmark = {
  id: string;
  label: string;
  url: string;
};

async function fetchStatus(): Promise<Status> {
  return invoke<Status>("get_status");
}

function escapeHtml(s: string): string {
  return s
    .replace(/&/g, "&amp;")
    .replace(/</g, "&lt;")
    .replace(/>/g, "&gt;")
    .replace(/"/g, "&quot;");
}

function brandBlock(): string {
  return `
    <header class="brand">
      <div class="brand-icon-shell">
        <img
          class="brand-icon"
          src="/icon.png"
          width="80"
          height="80"
          alt="PirateServer"
          decoding="async"
        />
        <span class="brand-badge" aria-hidden="true">1</span>
      </div>
      <div class="brand-text">
        <h1 class="brand-title">PirateServer</h1>
        <p class="brand-tagline">Local desktop — no cloud</p>
      </div>
    </header>
  `;
}

function serversCard(bookmarks: ServerBookmark[]): string {
  if (bookmarks.length === 0) {
    return `
      <div class="card">
        <div class="card-inner">
          <h2>Saved servers</h2>
          <p class="meta small">After you connect, URLs appear here. Use <strong>Use</strong> to switch the active endpoint.</p>
        </div>
      </div>
    `;
  }
  const rows = bookmarks
    .map(
      (b) => `
    <li class="bookmark-row">
      <div class="bm-head">
        <span class="bm-label">${escapeHtml(b.label)}</span>
      </div>
      <code class="bm-url">${escapeHtml(b.url)}</code>
      <div class="btn-row bm-actions">
        <button type="button" class="btn btn-ghost btn-sm" data-action="activate-server" data-url="${escapeHtml(b.url)}">Use</button>
        <button type="button" class="btn btn-ghost btn-sm danger" data-action="delete-server" data-id="${escapeHtml(b.id)}">Remove</button>
      </div>
    </li>`,
    )
    .join("");
  return `
    <div class="card">
      <div class="card-inner">
        <h2>Saved servers</h2>
        <ul class="bookmark-list">${rows}</ul>
      </div>
    </div>
  `;
}

function deployCard(
  saved: string | null,
  deployDir: string | null,
  deployFeedback: string | null,
): string {
  if (!saved) {
    return "";
  }
  const dirLine = deployDir
    ? `<p class="meta ok">Folder: <code>${escapeHtml(deployDir)}</code></p>`
    : `<p class="meta">Pick a build directory (e.g. containing <code>run.sh</code>).</p>`;
  const fb = deployFeedback
    ? `<p class="meta small">${escapeHtml(deployFeedback)}</p>`
    : "";
  return `
    <div class="card">
      <div class="card-inner">
        <h2>Deploy artifact</h2>
        <p class="meta small">Packs the folder as tar.gz and uploads over gRPC (same as CLI <code>client deploy</code>).</p>
        ${dirLine}
        ${fb}
        <div class="row">
          <label for="deploy-version">Version label</label>
          <input id="deploy-version" type="text" class="input-text" placeholder="v1.0.0" autocomplete="off" />
        </div>
        <div class="row">
          <label for="deploy-project">Project id</label>
          <input id="deploy-project" type="text" class="input-text" placeholder="default" value="default" autocomplete="off" />
        </div>
        <div class="btn-row">
          <button type="button" class="btn btn-ghost" data-action="pick-deploy-dir">Choose folder…</button>
          <button type="button" class="btn btn-primary" data-action="deploy-submit">Deploy</button>
        </div>
        <h3 class="subhead">Rollback</h3>
        <div class="row">
          <label for="rollback-version">Existing release version</label>
          <input id="rollback-version" type="text" class="input-text" placeholder="v0.9.0" autocomplete="off" />
        </div>
        <div class="btn-row">
          <button type="button" class="btn btn-ghost danger" data-action="rollback-submit">Rollback</button>
        </div>
      </div>
    </div>
  `;
}

function grpcCard(
  saved: string | null,
  live: GrpcConnectResult | null,
  grpcErr: string | null,
): string {
  const hint =
    "Paste the install JSON from deploy-server logs (<code>{\"token\",\"url\",\"pairing\"}</code>), or legacy <code>export GRPC_ENDPOINT=…</code> / a single URL if the server allows unauthenticated gRPC.";

  if (!saved) {
    return `
      <div class="card">
        <div class="card-inner">
          <h2>Deploy server (gRPC)</h2>
          <p class="meta">Not connected. Use the bundle from the machine where <code>deploy-server</code> runs.</p>
          <p class="meta small">${hint}</p>
          <button type="button" class="btn btn-primary" data-action="open-connect">
            Connect…
          </button>
        </div>
      </div>
    `;
  }

  const st = live;
  const statusLine = st
    ? `<p class="meta ok">Live: <code>${escapeHtml(st.state)}</code> · version <code>${escapeHtml(st.currentVersion || "—")}</code></p>`
    : `<p class="meta">Saved endpoint only (refresh status below).</p>`;
  const errLine = grpcErr
    ? `<p class="err-text">${escapeHtml(grpcErr)}</p>`
    : "";

  return `
    <div class="card">
      <div class="card-inner">
        <h2>Deploy server (gRPC)</h2>
        <p class="meta ok">Endpoint <code>${escapeHtml(saved)}</code></p>
        ${statusLine}
        ${errLine}
        <div class="btn-row">
          <button type="button" class="btn btn-primary" data-action="refresh-grpc">Refresh status</button>
          <button type="button" class="btn btn-ghost" data-action="open-connect">Change…</button>
          <button type="button" class="btn btn-ghost danger" data-action="disconnect">Disconnect</button>
        </div>
      </div>
    </div>
  `;
}

function modalMarkup(): string {
  return `
    <div class="modal-overlay" aria-hidden="true">
      <div class="modal" role="dialog" aria-modal="true" aria-labelledby="connect-title">
        <h3 id="connect-title" class="modal-title">Connect to deploy-server</h3>
        <p class="meta small">${escapeHtml(
          "Paste the install JSON from deploy-server (token, url, pairing), or legacy export lines / gRPC URL.",
        )}</p>
        <textarea
          class="input-bundle"
          id="bundle-input"
          rows="6"
          spellcheck="false"
          placeholder="export GRPC_ENDPOINT=http://127.0.0.1:50051&#10;export DASHBOARD_URL=http://127.0.0.1:18080&#10;…"
        ></textarea>
        <p class="form-err" id="bundle-err" hidden></p>
        <div class="btn-row">
          <button type="button" class="btn btn-primary" data-action="submit-connect">Connect</button>
          <button type="button" class="btn btn-ghost" data-action="close-modal-btn">Cancel</button>
        </div>
      </div>
    </div>
  `;
}

function render(
  root: HTMLElement,
  s: Status,
  opts: {
    savedEndpoint: string | null;
    grpcLive: GrpcConnectResult | null;
    grpcErr: string | null;
    modalOpen: boolean;
    deployDir: string | null;
    deployFeedback: string | null;
    bookmarks: ServerBookmark[];
  },
): void {
  const modal = opts.modalOpen ? modalMarkup() : "";
  root.innerHTML = `
    <div class="wrap">
      ${brandBlock()}
      ${grpcCard(opts.savedEndpoint, opts.grpcLive, opts.grpcErr)}
      ${serversCard(opts.bookmarks)}
      ${deployCard(opts.savedEndpoint, opts.deployDir, opts.deployFeedback)}
      <div class="card">
        <div class="card-inner">
          <h2>Status</h2>
          ${
            s.hosts_entry_ok
              ? `<p class="meta ok">Hosts file maps <code>${escapeHtml(s.hostname)}</code> to loopback.</p>`
              : `<p class="meta warn">No <code>${escapeHtml(s.hostname)}</code> in hosts (optional).</p>`
          }
          <p class="meta">This window is the app — no external browser needed.</p>
          <p class="shell-pill" title="UI shell">
            <span aria-hidden="true">✦</span> ${escapeHtml(s.shell)}
          </p>
        </div>
      </div>
    </div>
    ${modal}
  `;
}

function renderErr(root: HTMLElement, msg: string): void {
  root.innerHTML = `
    <div class="wrap">
      ${brandBlock()}
      <div class="card">
        <div class="card-inner">
          <h2>Something went wrong</h2>
          <p class="err-text">Could not load status: ${escapeHtml(msg)}</p>
          <p class="meta">Run inside the Tauri app (<code>npm run tauri:dev</code>).</p>
        </div>
      </div>
    </div>
  `;
}

function wireUi(
  root: HTMLElement,
  getState: () => {
    savedEndpoint: string | null;
    grpcLive: GrpcConnectResult | null;
    grpcErr: string | null;
    modalOpen: boolean;
    deployDir: string | null;
    deployFeedback: string | null;
    bookmarks: ServerBookmark[];
  },
  setState: (p: Partial<{
    savedEndpoint: string | null;
    grpcLive: GrpcConnectResult | null;
    grpcErr: string | null;
    modalOpen: boolean;
    deployDir: string | null;
    deployFeedback: string | null;
    bookmarks: ServerBookmark[];
  }>) => void,
  redraw: () => Promise<void>,
): void {
  root.onclick = (ev) => {
    const t = ev.target as HTMLElement;
    if (t.classList.contains("modal-overlay")) {
      setState({ modalOpen: false });
      void redraw();
      return;
    }
    const action = t.closest("[data-action]")?.getAttribute("data-action");
    if (!action) return;

    if (action === "open-connect") {
      setState({ modalOpen: true, grpcErr: null });
      void redraw();
      return;
    }
    if (action === "close-modal-btn") {
      setState({ modalOpen: false });
      void redraw();
      return;
    }
    if (action === "submit-connect") {
      const ta = document.getElementById("bundle-input") as HTMLTextAreaElement | null;
      const errEl = document.getElementById("bundle-err");
      const bundle = ta?.value?.trim() ?? "";
      if (!bundle) {
        if (errEl) {
          errEl.textContent = "Paste the server output or a gRPC URL.";
          errEl.hidden = false;
        }
        return;
      }
      void (async () => {
        try {
          const r = await invoke<GrpcConnectResult>("connect_grpc_bundle", { bundle });
          setState({
            savedEndpoint: r.endpoint,
            grpcLive: r,
            grpcErr: null,
            modalOpen: false,
            bookmarks: await invoke<ServerBookmark[]>("list_server_bookmarks"),
          });
          await redraw();
        } catch (e) {
          const msg = String(e);
          if (errEl) {
            errEl.textContent = msg;
            errEl.hidden = false;
          }
        }
      })();
      return;
    }
    if (action === "refresh-grpc") {
      void (async () => {
        try {
          const r = await invoke<GrpcConnectResult>("refresh_grpc_status");
          setState({ grpcLive: r, grpcErr: null });
          await redraw();
        } catch (e) {
          setState({ grpcErr: String(e) });
          await redraw();
        }
      })();
      return;
    }
    if (action === "disconnect") {
      void (async () => {
        try {
          await invoke("clear_grpc_connection");
          setState({
            savedEndpoint: null,
            grpcLive: null,
            grpcErr: null,
            deployDir: null,
            deployFeedback: null,
          });
          await redraw();
        } catch (e) {
          setState({ grpcErr: String(e) });
          await redraw();
        }
      })();
      return;
    }
    if (action === "pick-deploy-dir") {
      void (async () => {
        try {
          const p = await invoke<string | null>("pick_deploy_directory");
          setState({
            deployDir: p ?? null,
            deployFeedback: p ? null : "No folder selected.",
          });
          await redraw();
        } catch (e) {
          setState({ deployFeedback: String(e) });
          await redraw();
        }
      })();
      return;
    }
    if (action === "deploy-submit") {
      const st = getState();
      const dir = st.deployDir;
      const verEl = document.getElementById("deploy-version") as HTMLInputElement | null;
      const version = verEl?.value?.trim() ?? "";
      if (!dir) {
        setState({ deployFeedback: "Choose a folder first." });
        void redraw();
        return;
      }
      if (!version) {
        setState({ deployFeedback: "Enter a version label." });
        void redraw();
        return;
      }
      void (async () => {
        try {
          setState({ deployFeedback: "Uploading…" });
          await redraw();
          const projEl = document.getElementById(
            "deploy-project",
          ) as HTMLInputElement | null;
          const projectId = projEl?.value?.trim() || "default";
          await invoke("set_active_project", { project_id: projectId });
          const r = await invoke<DeployOutcome>("deploy_from_directory", {
            directory: dir,
            version,
            chunkSize: null,
          });
          let live: GrpcConnectResult | null = null;
          try {
            live = await invoke<GrpcConnectResult>("refresh_grpc_status");
          } catch {
            /* keep prior grpcLive */
          }
          setState({
            deployFeedback: `OK: ${r.status} → ${r.deployedVersion} (${r.artifactBytes} bytes, ${r.chunkCount} chunks)`,
            ...(live ? { grpcLive: live } : {}),
          });
          await redraw();
        } catch (e) {
          setState({ deployFeedback: String(e) });
          await redraw();
        }
      })();
      return;
    }
    if (action === "rollback-submit") {
      const rv = document.getElementById("rollback-version") as HTMLInputElement | null;
      const version = rv?.value?.trim() ?? "";
      if (!version) {
        setState({ deployFeedback: "Enter a version to roll back to." });
        void redraw();
        return;
      }
      void (async () => {
        try {
          setState({ deployFeedback: "Rolling back…" });
          await redraw();
          const r = await invoke<RollbackOutcome>("rollback_deploy", { version });
          setState({
            deployFeedback: `Rollback: ${r.status} → ${r.activeVersion}`,
            grpcLive: await invoke<GrpcConnectResult>("refresh_grpc_status").catch(() => null),
          });
          await redraw();
        } catch (e) {
          setState({ deployFeedback: String(e) });
          await redraw();
        }
      })();
      return;
    }
    if (action === "activate-server") {
      const url = t.closest("[data-url]")?.getAttribute("data-url");
      if (!url) return;
      void (async () => {
        try {
          const r = await invoke<GrpcConnectResult>("activate_server_bookmark", { url });
          setState({
            savedEndpoint: r.endpoint,
            grpcLive: r,
            grpcErr: null,
            bookmarks: await invoke<ServerBookmark[]>("list_server_bookmarks"),
          });
          await redraw();
        } catch (e) {
          setState({ grpcErr: String(e) });
          await redraw();
        }
      })();
      return;
    }
    if (action === "delete-server") {
      const id = t.closest("[data-id]")?.getAttribute("data-id");
      if (!id) return;
      void (async () => {
        try {
          await invoke("delete_server_bookmark", { id });
          setState({
            bookmarks: await invoke<ServerBookmark[]>("list_server_bookmarks"),
          });
          await redraw();
        } catch (e) {
          setState({ deployFeedback: String(e) });
          await redraw();
        }
      })();
      return;
    }
  };
}

const app = document.getElementById("app");
if (!app) {
  throw new Error("#app missing");
}
const root: HTMLElement = app;

const uiState: {
  savedEndpoint: string | null;
  grpcLive: GrpcConnectResult | null;
  grpcErr: string | null;
  modalOpen: boolean;
  deployDir: string | null;
  deployFeedback: string | null;
  bookmarks: ServerBookmark[];
} = {
  savedEndpoint: null,
  grpcLive: null,
  grpcErr: null,
  modalOpen: false,
  deployDir: null,
  deployFeedback: null,
  bookmarks: [],
};

async function redraw(): Promise<void> {
  const s = await fetchStatus();
  try {
    uiState.bookmarks = await invoke<ServerBookmark[]>("list_server_bookmarks");
  } catch {
    /* ignore */
  }
  render(root, s, uiState);
  wireUi(
    root,
    () => uiState,
    (p) => {
      Object.assign(uiState, p);
    },
    redraw,
  );
}

void fetchStatus()
  .then(async (s) => {
    uiState.savedEndpoint =
      (await invoke<string | null>("get_saved_grpc_endpoint")) ?? null;
    if (uiState.savedEndpoint) {
      try {
        uiState.grpcLive = await invoke<GrpcConnectResult>("refresh_grpc_status");
        uiState.grpcErr = null;
      } catch {
        uiState.grpcLive = null;
        uiState.grpcErr =
          "Could not reach saved endpoint (is deploy-server running?)";
      }
    }
    try {
      uiState.bookmarks = await invoke<ServerBookmark[]>("list_server_bookmarks");
    } catch {
      /* ignore */
    }
    render(root, s, uiState);
    wireUi(
      root,
      () => uiState,
      (p) => {
        Object.assign(uiState, p);
      },
      redraw,
    );
  })
  .catch((e: unknown) => renderErr(root, String(e)));
