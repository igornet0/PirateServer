import "./styles.css";

type Status = {
  bind_addr: string;
  port: number;
  hostname: string;
  hosts_entry_ok: boolean;
  preferred_url: string;
  fallback_url: string;
  ui_dir: string;
};

async function fetchStatus(): Promise<Status> {
  const r = await fetch("/api/v1/status");
  if (!r.ok) {
    throw new Error(`status ${r.status}`);
  }
  return r.json() as Promise<Status>;
}

function render(root: HTMLElement, s: Status): void {
  const hostsNote = s.hosts_entry_ok
    ? `<p class="meta ok">Hosts mapping for <code>${s.hostname}</code> is active.</p>`
    : `<p class="meta warn">Hosts mapping not applied (often needs admin). Use <code>${s.fallback_url}</code> or add <code>127.0.0.1 ${s.hostname}</code> to your hosts file.</p>`;

  root.innerHTML = `
    <div class="wrap">
      <h1>Pirate Client</h1>
      <p class="meta">Local desktop UI — no cloud. Bound to <code>${s.bind_addr}</code> only.</p>
      <div class="card">
        <p><strong>Open:</strong> <a href="${s.preferred_url}">${s.preferred_url}</a></p>
        <p class="meta">Fallback: <a href="${s.fallback_url}">${s.fallback_url}</a></p>
        ${hostsNote}
        <p class="meta">UI root: <code>${s.ui_dir}</code></p>
      </div>
    </div>
  `;
}

function renderErr(root: HTMLElement, msg: string): void {
  root.innerHTML = `
    <div class="wrap">
      <h1>Pirate Client</h1>
      <div class="card">
        <p class="warn">Could not load <code>/api/v1/status</code>: ${msg}</p>
      </div>
    </div>
  `;
}

const app = document.getElementById("app");
if (!app) {
  throw new Error("#app missing");
}

void fetchStatus()
  .then((s) => render(app, s))
  .catch((e: unknown) => renderErr(app, String(e)));
