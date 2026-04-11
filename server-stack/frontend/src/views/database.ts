import {
  deleteDataSource,
  fetchDataSources,
  fetchDatabaseColumns,
  fetchDatabaseInfo,
  fetchDatabaseRelationships,
  fetchDatabaseSchemas,
  fetchDatabaseTableRows,
  fetchDatabaseTables,
  fetchSmbBrowse,
  postDataSourceConnection,
  postDataSourceSmb,
} from "../api/client.js";
import type { DataSourceItemView, DatabaseInfoView, ForeignKeyRow } from "../api/types.js";
import { ApiRequestError } from "../api/types.js";
import { t } from "../i18n/index.js";

type DbSub = "overview" | "browse" | "relations";

const DB_SOURCE_STORAGE = "deploy.dbSource";

const state = {
  sources: [] as DataSourceItemView[],
  selectedSourceId: "postgresql",
  pgConfigured: false,
  info: null as DatabaseInfoView | null,
  schemas: [] as string[],
  schema: "public",
  table: null as string | null,
  sub: "overview" as DbSub,
  browseTab: "columns" as "columns" | "data",
  dataOffset: 0,
  dataLimit: 100,
  smbBrowsePath: "",
};

function svgIconPostgres(): string {
  return `<svg class="db-source-svg" width="22" height="22" viewBox="0 0 24 24" fill="none" xmlns="http://www.w3.org/2000/svg" aria-hidden="true"><path d="M12 9c2.5 0 4.5-1.1 4.5-2.5S14.5 4 12 4 7.5 5.1 7.5 6.5 9.5 9 12 9z" stroke="currentColor" stroke-width="1.5"/><path d="M7.5 6.5V11c0 2.2 2.5 4 4.5 4s4.5-1.8 4.5-4V6.5" stroke="currentColor" stroke-width="1.5"/><path d="M7.5 11v6c0 1.2 1.5 2.2 3.5 2.8V18" stroke="currentColor" stroke-width="1.5"/></svg>`;
}

function svgIconSmb(): string {
  return `<svg class="db-source-svg" width="22" height="22" viewBox="0 0 24 24" fill="none" xmlns="http://www.w3.org/2000/svg" aria-hidden="true"><path d="M12 9V4H5v16h14v-7" stroke="currentColor" stroke-width="1.5" stroke-linecap="round"/><path d="M12 9h7l4 4v7H5" stroke="currentColor" stroke-width="1.5" stroke-linecap="round" stroke-linejoin="round"/><path d="M9 15h6M9 18h4" stroke="currentColor" stroke-width="1.5" stroke-linecap="round"/></svg>`;
}

function svgIconDatabase(): string {
  return `<svg class="db-source-svg" width="22" height="22" viewBox="0 0 24 24" fill="none" xmlns="http://www.w3.org/2000/svg" aria-hidden="true"><ellipse cx="12" cy="6" rx="7" ry="3" stroke="currentColor" stroke-width="1.5"/><path d="M5 6v6c0 1.7 3.1 3 7 3s7-1.3 7-3V6" stroke="currentColor" stroke-width="1.5"/><path d="M5 12v6c0 1.7 3.1 3 7 3s7-1.3 7-3v-6" stroke="currentColor" stroke-width="1.5"/></svg>`;
}

const DEFAULT_CONN_PORTS: Record<string, number> = {
  clickhouse: 8123,
  oracle: 1521,
  mysql: 3306,
  postgresql: 5432,
  mssql: 1433,
  mongodb: 27017,
  redis: 6379,
};

const CONNECTION_KINDS = new Set([
  "clickhouse",
  "oracle",
  "mysql",
  "postgresql",
  "mssql",
  "mongodb",
  "redis",
]);

function isConnSourceKind(kind: string): boolean {
  return CONNECTION_KINDS.has(kind);
}

function kindShort(kind: string): string {
  switch (kind) {
    case "smb":
      return t("database.kindShort.smb");
    case "clickhouse":
      return t("database.kindShort.clickhouse");
    case "oracle":
      return t("database.kindShort.oracle");
    case "mysql":
      return t("database.kindShort.mysql");
    case "postgresql":
      return t("database.kindShort.postgresql");
    case "mssql":
      return t("database.kindShort.mssql");
    case "mongodb":
      return t("database.kindShort.mongodb");
    case "redis":
      return t("database.kindShort.redis");
    default:
      return kind;
  }
}

function sourceOptionLabel(src: DataSourceItemView): string {
  if (src.id === "postgresql" && src.kind === "postgresql") {
    return t("database.kindShort.postgresql");
  }
  return `${kindShort(src.kind)} · ${src.label}`;
}

function iconForSourceKind(kind: string): string {
  if (kind === "postgresql") {
    return svgIconPostgres();
  }
  if (kind === "smb") {
    return svgIconSmb();
  }
  return svgIconDatabase();
}

function formatBytes(n: number): string {
  if (n < 1024) {
    return `${n} B`;
  }
  const units = ["KB", "MB", "GB", "TB"];
  let v = n;
  let i = 0;
  while (v >= 1024 && i < units.length - 1) {
    v /= 1024;
    i += 1;
  }
  return `${i === 0 ? v : v.toFixed(1)} ${units[i]}`;
}

function formatErr(e: unknown): string {
  if (e instanceof ApiRequestError) {
    return `${e.message} (${e.status}${e.code ? ` / ${e.code}` : ""})`;
  }
  return String(e);
}

function escapeHtml(s: string): string {
  return s
    .replaceAll("&", "&amp;")
    .replaceAll("<", "&lt;")
    .replaceAll(">", "&gt;")
    .replaceAll('"', "&quot;");
}

function mermaidEntityId(schema: string, table: string): string {
  return `${schema}_${table}`.replace(/[^a-zA-Z0-9_]/g, "_");
}

function buildErDiagram(fks: ForeignKeyRow[]): string | null {
  if (fks.length === 0) {
    return null;
  }
  const lines: string[] = ["erDiagram"];
  for (const fk of fks) {
    const child = mermaidEntityId(fk.table_schema, fk.table_name);
    const parent = mermaidEntityId(fk.foreign_table_schema, fk.foreign_table_name);
    const lab = fk.column_name.replace(/"/g, "'");
    lines.push(`  ${parent} ||--o{ ${child} : "${lab}"`);
  }
  return lines.join("\n");
}

async function renderMermaid(container: HTMLElement, code: string | null): Promise<void> {
  if (code === null) {
    container.innerHTML = `<p class="muted">—</p>`;
    return;
  }
  try {
    const mermaid = (await import("mermaid")).default;
    mermaid.initialize({
      startOnLoad: false,
      theme: "dark",
      securityLevel: "loose",
      fontFamily: "var(--font-sans)",
    });
    const id = `mmd-db-${Date.now()}`;
    const { svg } = await mermaid.render(id, code);
    container.innerHTML = `<div class="db-mermaid-wrap">${svg}</div>`;
  } catch {
    container.innerHTML = `<p class="muted">${escapeHtml(t("database.mermaidError"))}</p>`;
  }
}

function selectedSource(): DataSourceItemView | undefined {
  return state.sources.find((s) => s.id === state.selectedSourceId);
}

function isPgSelected(): boolean {
  return state.selectedSourceId === "postgresql";
}

function smbParentPath(p: string): string {
  if (!p) {
    return "";
  }
  const parts = p.split("/").filter(Boolean);
  parts.pop();
  return parts.join("/");
}

function smbChildPath(p: string, name: string): string {
  return p ? `${p}/${name}` : name;
}

function pickDefaultSourceId(sources: DataSourceItemView[], pgConfigured: boolean): string {
  if (pgConfigured && sources.some((s) => s.kind === "postgresql")) {
    return "postgresql";
  }
  const first = sources.find((s) => s.kind !== "postgresql");
  return first?.id ?? "postgresql";
}

function readStoredSourceId(): string | null {
  try {
    const s = sessionStorage.getItem(DB_SOURCE_STORAGE);
    return s?.trim() || null;
  } catch {
    return null;
  }
}

function writeStoredSourceId(id: string): void {
  try {
    sessionStorage.setItem(DB_SOURCE_STORAGE, id);
  } catch {
    /* ignore */
  }
}

function syncSourceToolbar(): void {
  const toolbar = document.getElementById("db-source-toolbar");
  const sel = document.getElementById("db-source-select") as HTMLSelectElement | null;
  const iconWrap = document.getElementById("db-source-icon");
  if (!toolbar || !sel || !iconWrap) {
    return;
  }
  if (state.sources.length === 0) {
    toolbar.hidden = true;
    return;
  }
  toolbar.hidden = false;
  sel.innerHTML = "";
  for (const src of state.sources) {
    const opt = document.createElement("option");
    opt.value = src.id;
    opt.textContent = sourceOptionLabel(src);
    sel.appendChild(opt);
  }
  if (!state.sources.some((s) => s.id === state.selectedSourceId)) {
    state.selectedSourceId = pickDefaultSourceId(state.sources, state.pgConfigured);
    writeStoredSourceId(state.selectedSourceId);
  }
  sel.value = state.selectedSourceId;
  const selSrc = state.sources.find((s) => s.id === state.selectedSourceId);
  iconWrap.innerHTML = selSrc ? iconForSourceKind(selSrc.kind) : svgIconDatabase();
}

function renderOverview(root: HTMLElement): void {
  const host = root.querySelector<HTMLElement>("#db-sub-panel");
  if (!host || !state.info) {
    return;
  }
  const d = state.info;
  const cards: [string, string][] = [];
  if (d.connection_display) {
    cards.push([t("database.connection"), d.connection_display]);
  }
  if (d.server_version) {
    cards.push([t("database.version"), d.server_version]);
  }
  if (d.database_name) {
    cards.push([t("database.dbName"), d.database_name]);
  }
  if (d.session_user) {
    cards.push([t("database.user"), d.session_user]);
  }
  if (d.database_size_bytes != null) {
    cards.push([t("database.size"), formatBytes(d.database_size_bytes)]);
  }
  if (d.active_connections != null) {
    cards.push([t("database.connections"), String(d.active_connections)]);
  }
  const grid = cards
    .map(
      ([k, v]) => `
    <article class="db-metric-card">
      <div class="db-metric-kicker">${escapeHtml(k)}</div>
      <div class="db-metric-value">${escapeHtml(v)}</div>
    </article>`,
    )
    .join("");
  host.innerHTML = `<div class="db-metric-grid">${grid}</div>
    <p class="muted db-pg-builtin-hint">${escapeHtml(t("database.pgBuiltinHint"))}</p>`;
}

function renderSmbOverview(root: HTMLElement): void {
  const host = root.querySelector<HTMLElement>("#db-sub-panel");
  if (!host) {
    return;
  }
  const src = selectedSource();
  if (!src || src.kind !== "smb") {
    host.innerHTML = `<p class="muted">—</p>`;
    return;
  }
  const cards: [string, string][] = [];
  cards.push([t("database.smbOverviewMount"), src.mount_point ?? "—"]);
  cards.push([t("database.smbOverviewState"), src.mount_state ?? "—"]);
  if (src.smb_host) {
    cards.push([t("database.smbOverviewHost"), src.smb_host]);
  }
  if (src.smb_share) {
    cards.push([t("database.smbOverviewShare"), src.smb_share]);
  }
  if (src.smb_subpath != null && src.smb_subpath !== "") {
    cards.push([t("database.smbOverviewFolder"), src.smb_subpath]);
  }
  if (src.last_error) {
    cards.push([t("database.smbOverviewError"), src.last_error]);
  }
  const grid = cards
    .map(
      ([k, v]) => `
    <article class="db-metric-card">
      <div class="db-metric-kicker">${escapeHtml(k)}</div>
      <div class="db-metric-value">${escapeHtml(v)}</div>
    </article>`,
    )
    .join("");
  host.innerHTML = `<div class="db-metric-grid">${grid}</div>
    <div class="db-smb-actions">
      <button type="button" class="danger" id="btn-smb-remove">${escapeHtml(t("database.smbRemove"))}</button>
    </div>`;
  document.getElementById("btn-smb-remove")?.addEventListener("click", () => {
    void (async () => {
      if (!window.confirm(t("database.smbRemoveConfirm"))) {
        return;
      }
      try {
        await deleteDataSource(src.id);
        writeStoredSourceId(pickDefaultSourceId(state.sources.filter((s) => s.id !== src.id), state.pgConfigured));
        await loadDatabaseInfo();
      } catch (e) {
        alert(`${t("database.smbRemoveError")} ${formatErr(e)}`);
      }
    })();
  });
}

function isConnSource(): boolean {
  const s = selectedSource();
  return s != null && isConnSourceKind(s.kind);
}

function renderConnOverview(root: HTMLElement): void {
  const host = root.querySelector<HTMLElement>("#db-sub-panel");
  if (!host) {
    return;
  }
  const src = selectedSource();
  if (!src || !isConnSourceKind(src.kind)) {
    host.innerHTML = `<p class="muted">—</p>`;
    return;
  }
  const cfg = src.config_json;
  const cards: [string, string][] = [];
  cards.push([
    t("database.connFieldHost"),
    cfg != null && typeof (cfg as { host?: unknown }).host === "string"
      ? (cfg as { host: string }).host
      : "—",
  ]);
  cards.push([
    t("database.connFieldPort"),
    cfg != null && typeof (cfg as { port?: unknown }).port === "number"
      ? String((cfg as { port: number }).port)
      : "—",
  ]);
  if (cfg != null && typeof (cfg as { database?: unknown }).database === "string") {
    const db = (cfg as { database: string }).database;
    if (db) {
      cards.push([t("database.connFieldDatabase"), db]);
    }
  }
  if (cfg != null && typeof (cfg as { username?: unknown }).username === "string") {
    const u = (cfg as { username: string }).username;
    if (u) {
      cards.push([t("database.connFieldUsername"), u]);
    }
  }
  cards.push([t("database.connOverviewState"), src.mount_state ?? "—"]);
  cards.push([
    t("database.connOverviewCred"),
    src.has_credentials ? t("database.yes") : t("database.no"),
  ]);
  if (cfg != null && typeof (cfg as { ssl?: unknown }).ssl === "boolean") {
    cards.push([
      t("database.connFieldSsl"),
      (cfg as { ssl: boolean }).ssl ? t("database.yes") : t("database.no"),
    ]);
  }
  if (src.last_error) {
    cards.push([t("database.smbOverviewError"), src.last_error]);
  }
  const grid = cards
    .map(
      ([k, v]) => `
    <article class="db-metric-card">
      <div class="db-metric-kicker">${escapeHtml(k)}</div>
      <div class="db-metric-value">${escapeHtml(v)}</div>
    </article>`,
    )
    .join("");
  host.innerHTML = `<div class="db-metric-grid">${grid}</div>
    <div class="db-smb-actions">
      <button type="button" class="danger" id="btn-conn-remove">${escapeHtml(t("database.connRemove"))}</button>
    </div>`;
  document.getElementById("btn-conn-remove")?.addEventListener("click", () => {
    void (async () => {
      if (!window.confirm(t("database.connRemoveConfirm"))) {
        return;
      }
      try {
        await deleteDataSource(src.id);
        writeStoredSourceId(pickDefaultSourceId(state.sources.filter((s) => s.id !== src.id), state.pgConfigured));
        await loadDatabaseInfo();
      } catch (e) {
        alert(`${t("database.connRemoveError")} ${formatErr(e)}`);
      }
    })();
  });
}

function renderConnBrowse(root: HTMLElement): void {
  const host = root.querySelector<HTMLElement>("#db-sub-panel");
  if (!host) {
    return;
  }
  host.innerHTML = `<p class="muted">${escapeHtml(t("database.connBrowseHint"))}</p>`;
}

function renderConnRelations(root: HTMLElement): void {
  const host = root.querySelector<HTMLElement>("#db-sub-panel");
  if (!host) {
    return;
  }
  host.innerHTML = `<p class="muted">${escapeHtml(t("database.connRelationsHint"))}</p>`;
}

async function renderBrowse(root: HTMLElement): Promise<void> {
  const host = root.querySelector<HTMLElement>("#db-sub-panel");
  if (!host) {
    return;
  }
  host.innerHTML = `<div class="db-browse-layout">
    <aside class="db-browse-aside" aria-label="${escapeHtml(t("database.tablesHeading"))}">
      <label class="db-field-label" for="db-schema-select">${escapeHtml(t("database.schemaLabel"))}</label>
      <select id="db-schema-select" class="db-schema-select"></select>
      <div class="db-aside-heading">${escapeHtml(t("database.tablesHeading"))}</div>
      <div id="db-table-list" class="db-table-list"></div>
    </aside>
    <div class="db-browse-main">
      <div class="db-innertabs" role="tablist">
        <button type="button" class="db-innertab" data-db-browse="columns" role="tab">${escapeHtml(t("database.tabSchema"))}</button>
        <button type="button" class="db-innertab" data-db-browse="data" role="tab">${escapeHtml(t("database.tabData"))}</button>
      </div>
      <div id="db-browse-detail" class="db-browse-detail"></div>
    </div>
  </div>`;

  const sel = host.querySelector<HTMLSelectElement>("#db-schema-select");
  const list = host.querySelector<HTMLElement>("#db-table-list");
  if (!sel || !list) {
    return;
  }

  for (const s of state.schemas) {
    const opt = document.createElement("option");
    opt.value = s;
    opt.textContent = s;
    if (s === state.schema) {
      opt.selected = true;
    }
    sel.appendChild(opt);
  }

  sel.addEventListener("change", () => {
    state.schema = sel.value;
    state.table = null;
    state.dataOffset = 0;
    void fillBrowseTables(host);
  });

  host.querySelectorAll<HTMLButtonElement>("[data-db-browse]").forEach((btn) => {
    btn.addEventListener("click", () => {
      const mode = btn.getAttribute("data-db-browse") as "columns" | "data";
      state.browseTab = mode;
      state.dataOffset = 0;
      host.querySelectorAll("[data-db-browse]").forEach((b) => {
        b.classList.toggle("is-active", b === btn);
      });
      void renderBrowseDetail(host);
    });
  });

  try {
    await fillBrowseTables(host);
  } catch (e) {
    host.querySelector("#db-browse-detail")!.innerHTML = `<p class="muted">${escapeHtml(formatErr(e))}</p>`;
  }

  const activeInner = state.browseTab === "data" ? "[data-db-browse=data]" : "[data-db-browse=columns]";
  host.querySelector(activeInner)?.classList.add("is-active");
}

async function fillBrowseTables(host: HTMLElement): Promise<void> {
  const list = host.querySelector<HTMLElement>("#db-table-list");
  const detail = host.querySelector<HTMLElement>("#db-browse-detail");
  if (!list || !detail) {
    return;
  }
  list.innerHTML = `<span class="muted">${escapeHtml(t("database.loading"))}</span>`;
  const res = await fetchDatabaseTables(state.schema);
  const tables = res.tables ?? [];
  if (tables.length === 0) {
    list.innerHTML = `<span class="muted">${escapeHtml(t("database.noTables"))}</span>`;
    state.table = null;
    detail.innerHTML = `<p class="muted">${escapeHtml(t("database.selectTableHint"))}</p>`;
    return;
  }
  if (!state.table || !tables.some((x) => x.name === state.table)) {
    state.table = tables[0]!.name;
  }
  list.innerHTML = tables
    .map((tb) => {
      const active = tb.name === state.table ? " is-active" : "";
      return `<button type="button" class="db-table-pill${active}" data-db-table="${escapeHtml(tb.name)}">
        <span class="db-table-pill-name">${escapeHtml(tb.name)}</span>
        <span class="db-table-pill-meta">${escapeHtml(tb.table_type)}${tb.row_estimate != null ? ` · ~${tb.row_estimate}` : ""}</span>
      </button>`;
    })
    .join("");

  list.querySelectorAll<HTMLButtonElement>("[data-db-table]").forEach((btn) => {
    btn.addEventListener("click", () => {
      state.table = btn.getAttribute("data-db-table");
      state.dataOffset = 0;
      list.querySelectorAll(".db-table-pill").forEach((p) => p.classList.remove("is-active"));
      btn.classList.add("is-active");
      void renderBrowseDetail(host);
    });
  });

  await renderBrowseDetail(host);
}

async function renderBrowseDetail(host: HTMLElement): Promise<void> {
  const detail = host.querySelector<HTMLElement>("#db-browse-detail");
  if (!detail || !state.table) {
    return;
  }
  detail.innerHTML = `<div class="db-detail-loading">${escapeHtml(t("database.loading"))}</div>`;
  try {
    if (state.browseTab === "columns") {
      const c = await fetchDatabaseColumns(state.schema, state.table);
      const rows = c.columns ?? [];
      const thead = `<tr><th>${escapeHtml(t("database.colName"))}</th><th>${escapeHtml(t("database.colType"))}</th><th>${escapeHtml(t("database.colNullable"))}</th><th>${escapeHtml(t("database.colDefault"))}</th></tr>`;
      const tbody = rows
        .map(
          (col) =>
            `<tr><td><code>${escapeHtml(col.column_name)}</code></td><td>${escapeHtml(col.data_type)}</td><td>${escapeHtml(col.is_nullable)}</td><td class="db-cell-muted">${col.column_default != null ? escapeHtml(col.column_default) : "—"}</td></tr>`,
        )
        .join("");
      detail.innerHTML = `<div class="db-table-scroll"><table class="db-data-table db-schema-table"><thead>${thead}</thead><tbody>${tbody}</tbody></table></div>`;
    } else {
      const r = await fetchDatabaseTableRows(state.schema, state.table, {
        limit: state.dataLimit,
        offset: state.dataOffset,
      });
      const raw = r.preview?.rows;
      const arr = Array.isArray(raw) ? raw : [];
      if (arr.length === 0) {
        detail.innerHTML = `<p class="muted">—</p>`;
        return;
      }
      const first = arr[0] as Record<string, unknown>;
      const keys = Object.keys(first);
      const thead = `<tr>${keys.map((k) => `<th>${escapeHtml(k)}</th>`).join("")}</tr>`;
      const tbody = arr
        .map((row) => {
          const o = row as Record<string, unknown>;
          return `<tr>${keys.map((k) => `<td>${escapeHtml(cellStr(o[k]))}</td>`).join("")}</tr>`;
        })
        .join("");
      const hint = t("database.rowsShown", {
        limit: state.dataLimit,
        offset: state.dataOffset,
      });
      detail.innerHTML = `<p class="muted db-rows-hint">${escapeHtml(hint)}</p><div class="db-table-scroll"><table class="db-data-table"><thead>${thead}</thead><tbody>${tbody}</tbody></table></div>`;
    }
  } catch (e) {
    detail.innerHTML = `<p class="muted">${escapeHtml(formatErr(e))}</p>`;
  }
}

function cellStr(v: unknown): string {
  if (v === null || v === undefined) {
    return "";
  }
  if (typeof v === "object") {
    return JSON.stringify(v);
  }
  return String(v);
}

async function renderSmbBrowse(root: HTMLElement): Promise<void> {
  const host = root.querySelector<HTMLElement>("#db-sub-panel");
  if (!host) {
    return;
  }
  const src = selectedSource();
  if (!src || src.kind !== "smb") {
    host.innerHTML = `<p class="muted">—</p>`;
    return;
  }
  host.innerHTML = `<div class="db-smb-browse">
    <div class="db-smb-browse-toolbar">
      <button type="button" class="ghost" id="btn-smb-up" ${state.smbBrowsePath ? "" : "disabled"}>${escapeHtml(t("database.smbUp"))}</button>
      <code class="db-smb-path" id="db-smb-path">${escapeHtml(state.smbBrowsePath || "/")}</code>
    </div>
    <div id="db-smb-browse-body" class="db-smb-browse-body muted">${escapeHtml(t("database.loading"))}</div>
  </div>`;

  document.getElementById("btn-smb-up")?.addEventListener("click", () => {
    state.smbBrowsePath = smbParentPath(state.smbBrowsePath);
    void renderSmbBrowse(root);
  });

  const body = host.querySelector<HTMLElement>("#db-smb-browse-body");
  if (!body) {
    return;
  }
  try {
    const view = await fetchSmbBrowse(src.id, state.smbBrowsePath);
    const entries = view.entries ?? [];
    if (entries.length === 0) {
      body.innerHTML = `<p class="muted">${escapeHtml(t("database.smbEmpty"))}</p>`;
      return;
    }
    const thead = `<tr><th>${escapeHtml(t("database.smbColKind"))}</th><th>${escapeHtml(t("database.smbColName"))}</th><th>${escapeHtml(t("database.smbColSize"))}</th></tr>`;
    const tbody = entries
      .map((e) => {
        const kind = e.is_dir ? t("database.smbKindDir") : t("database.smbKindFile");
        const sz =
          e.is_dir || e.size == null ? "—" : formatBytes(e.size);
        const nameEnc = encodeURIComponent(e.name);
        return `<tr class="db-smb-row" data-smb-name="${nameEnc}" data-smb-dir="${e.is_dir ? "1" : "0"}">
          <td>${escapeHtml(kind)}</td>
          <td><button type="button" class="db-smb-name-btn">${escapeHtml(e.name)}</button></td>
          <td>${escapeHtml(sz)}</td>
        </tr>`;
      })
      .join("");
    body.innerHTML = `<div class="db-table-scroll"><table class="db-data-table"><thead>${thead}</thead><tbody>${tbody}</tbody></table></div>`;
    body.querySelectorAll<HTMLButtonElement>(".db-smb-name-btn").forEach((btn) => {
      btn.addEventListener("click", () => {
        const tr = btn.closest("tr");
        const isDir = tr?.getAttribute("data-smb-dir") === "1";
        const raw = tr?.getAttribute("data-smb-name");
        let name = "";
        try {
          name = raw ? decodeURIComponent(raw) : "";
        } catch {
          name = raw ?? "";
        }
        if (!isDir || !name) {
          return;
        }
        state.smbBrowsePath = smbChildPath(state.smbBrowsePath, name);
        void renderSmbBrowse(root);
      });
    });
  } catch (e) {
    body.innerHTML = `<p class="muted">${escapeHtml(formatErr(e))}</p>`;
  }
}

async function renderRelations(root: HTMLElement): Promise<void> {
  const host = root.querySelector<HTMLElement>("#db-sub-panel");
  if (!host) {
    return;
  }
  host.innerHTML = `<div class="db-rel-loading muted">${escapeHtml(t("database.loading"))}</div>`;
  try {
    const rel = await fetchDatabaseRelationships();
    const fks = rel.foreign_keys ?? [];
    const diagram = buildErDiagram(fks);
    const tableRows = fks
      .map(
        (fk) =>
          `<tr><td><code>${escapeHtml(fk.table_schema)}.${escapeHtml(fk.table_name)}</code></td><td><code>${escapeHtml(fk.column_name)}</code></td><td><code>${escapeHtml(fk.foreign_table_schema)}.${escapeHtml(fk.foreign_table_name)}</code></td><td><code>${escapeHtml(fk.foreign_column_name)}</code></td></tr>`,
      )
      .join("");
    host.innerHTML = `
      <div class="db-rel-grid">
        <section class="db-rel-section">
          <h3 class="db-rel-title">${escapeHtml(t("database.fkDiagramTitle"))}</h3>
          <div id="db-mermaid-host" class="db-mermaid-host"></div>
        </section>
        <section class="db-rel-section">
          <h3 class="db-rel-title">${escapeHtml(t("database.fkMapTitle"))}</h3>
          <div class="db-table-scroll">
            <table class="db-data-table">
              <thead><tr><th>${escapeHtml(t("database.fkFrom"))}</th><th>${escapeHtml(t("database.colName"))}</th><th>${escapeHtml(t("database.fkTo"))}</th><th>${escapeHtml(t("database.colName"))}</th></tr></thead>
              <tbody>${tableRows || `<tr><td colspan="4" class="muted">—</td></tr>`}</tbody>
            </table>
          </div>
        </section>
      </div>`;
    const mh = host.querySelector<HTMLElement>("#db-mermaid-host");
    if (mh) {
      await renderMermaid(mh, diagram);
    }
  } catch (e) {
    host.innerHTML = `<p class="muted">${escapeHtml(formatErr(e))}</p>`;
  }
}

function renderSmbRelations(root: HTMLElement): void {
  const host = root.querySelector<HTMLElement>("#db-sub-panel");
  if (!host) {
    return;
  }
  host.innerHTML = `<p class="muted">${escapeHtml(t("database.smbRelationsHint"))}</p>`;
}

function updateSubtabUi(root: HTMLElement, smb: boolean, conn: boolean): void {
  root.querySelectorAll<HTMLButtonElement>("[data-db-sub]").forEach((btn) => {
    const sub = btn.getAttribute("data-db-sub") as DbSub;
    const sel = sub === state.sub;
    btn.classList.toggle("is-active", sel);
    btn.setAttribute("aria-selected", sel ? "true" : "false");
  });
  const browseBtn = root.querySelector<HTMLButtonElement>('[data-db-sub="browse"]');
  if (browseBtn) {
    if (smb) {
      browseBtn.textContent = t("database.subFiles");
    } else if (conn) {
      browseBtn.textContent = t("database.subDetails");
    } else {
      browseBtn.textContent = t("database.subBrowse");
    }
  }
}

function renderSubPanel(root: HTMLElement): void {
  const conn = isConnSource();
  const smb = !isPgSelected() && !conn;
  updateSubtabUi(root, smb, conn);
  const panel = root.querySelector<HTMLElement>("#db-sub-panel");
  if (!panel) {
    return;
  }
  if (conn) {
    if (state.sub === "overview") {
      renderConnOverview(root);
      return;
    }
    if (state.sub === "browse") {
      renderConnBrowse(root);
      return;
    }
    renderConnRelations(root);
    return;
  }
  if (smb) {
    if (state.sub === "overview") {
      renderSmbOverview(root);
      return;
    }
    if (state.sub === "browse") {
      void renderSmbBrowse(root);
      return;
    }
    renderSmbRelations(root);
    return;
  }
  if (state.sub === "overview") {
    renderOverview(root);
    return;
  }
  if (state.sub === "browse") {
    void renderBrowse(root);
    return;
  }
  void renderRelations(root);
}

function mountExplorer(root: HTMLElement, mode: "pg" | "smb" | "conn"): void {
  const smb = mode === "smb";
  const conn = mode === "conn";
  let browseLabel = t("database.subBrowse");
  if (smb) {
    browseLabel = t("database.subFiles");
  }
  if (conn) {
    browseLabel = t("database.subDetails");
  }
  root.innerHTML = `
    <nav class="db-subtabs" role="tablist" aria-label="Database">
      <button type="button" class="db-subtab" role="tab" data-db-sub="overview">${escapeHtml(t("database.subOverview"))}</button>
      <button type="button" class="db-subtab" role="tab" data-db-sub="browse">${escapeHtml(browseLabel)}</button>
      <button type="button" class="db-subtab" role="tab" data-db-sub="relations">${escapeHtml(t("database.subRelations"))}</button>
    </nav>
    <div id="db-sub-panel" class="db-sub-panel"></div>`;

  root.querySelectorAll<HTMLButtonElement>("[data-db-sub]").forEach((btn) => {
    btn.addEventListener("click", () => {
      state.sub = btn.getAttribute("data-db-sub") as DbSub;
      renderSubPanel(root);
    });
  });

  state.sub = "overview";
  updateSubtabUi(root, smb, conn);
  renderSubPanel(root);
}

function renderPanelBody(): void {
  const root = document.getElementById("database-content");
  if (!root) {
    return;
  }
  if (isPgSelected()) {
    if (!state.pgConfigured) {
      root.innerHTML = `<p class="muted db-unconfigured">${escapeHtml(t("database.notConfigured"))}</p>`;
      return;
    }
    const inner = document.createElement("div");
    inner.className = "db-explorer-inner";
    root.innerHTML = "";
    root.appendChild(inner);
    mountExplorer(inner, "pg");
    return;
  }
  if (isConnSource()) {
    const inner = document.createElement("div");
    inner.className = "db-explorer-inner";
    root.innerHTML = "";
    root.appendChild(inner);
    mountExplorer(inner, "conn");
    return;
  }
  const inner = document.createElement("div");
  inner.className = "db-explorer-inner";
  root.innerHTML = "";
  root.appendChild(inner);
  mountExplorer(inner, "smb");
}

export async function loadDatabaseInfo(): Promise<void> {
  const root = document.getElementById("database-content");
  if (!root) {
    return;
  }
  root.innerHTML = `<div class="db-loading muted">${escapeHtml(t("database.loading"))}</div>`;
  try {
    const [list, info] = await Promise.all([fetchDataSources(), fetchDatabaseInfo()]);
    state.sources = list.sources ?? [];
    state.pgConfigured = info.configured;
    state.info = info;
    if (state.pgConfigured) {
      const sch = await fetchDatabaseSchemas();
      const schemaNames = (sch.schemas ?? []).map((s) => s.name);
      state.schemas = schemaNames.length > 0 ? schemaNames : ["public"];
      state.schema = state.schemas.includes("public") ? "public" : state.schemas[0]!;
    } else {
      state.schemas = ["public"];
      state.schema = "public";
    }
    state.table = null;
    state.sub = "overview";
    state.browseTab = "columns";
    state.dataOffset = 0;
    state.smbBrowsePath = "";

    const stored = readStoredSourceId();
    if (stored && state.sources.some((s) => s.id === stored)) {
      state.selectedSourceId = stored;
    } else {
      state.selectedSourceId = pickDefaultSourceId(state.sources, state.pgConfigured);
      writeStoredSourceId(state.selectedSourceId);
    }

    syncSourceToolbar();
    renderPanelBody();
  } catch (e) {
    root.innerHTML = `<p class="muted">${escapeHtml(formatErr(e))}</p>`;
    state.info = null;
    state.sources = [];
  }
}

export function refreshDatabaseFromLocale(): void {
  const kindEl = document.getElementById("conn-kind") as HTMLInputElement | null;
  if (kindEl?.value === "postgresql") {
    syncConnPgInstallHint("postgresql");
  }
  if (document.getElementById("tab-database")?.getAttribute("aria-selected") === "true") {
    void loadDatabaseInfo();
  }
}

function bindSourceSelect(): void {
  document.getElementById("db-source-select")?.addEventListener("change", (ev) => {
    const v = (ev.target as HTMLSelectElement).value;
    state.selectedSourceId = v;
    writeStoredSourceId(v);
    state.sub = "overview";
    state.smbBrowsePath = "";
    state.table = null;
    syncSourceToolbar();
    renderPanelBody();
  });
}

function showWizardPick(): void {
  document.getElementById("wizard-step-pick")?.removeAttribute("hidden");
  document.getElementById("wizard-step-form")?.setAttribute("hidden", "");
}

function showWizardForm(): void {
  document.getElementById("wizard-step-pick")?.setAttribute("hidden", "");
  document.getElementById("wizard-step-form")?.removeAttribute("hidden");
}

function syncConnPgInstallHint(kind: string | null): void {
  const el = document.getElementById("conn-pg-install-hint");
  if (!el) {
    return;
  }
  if (kind === "postgresql") {
    el.textContent = t("database.connPgInstallHint");
    el.removeAttribute("hidden");
  } else {
    el.setAttribute("hidden", "");
    el.textContent = "";
  }
}

function bindDatasourceWizard(): void {
  const dlg = document.getElementById("dialog-datasource-wizard") as HTMLDialogElement | null;
  const smbForm = document.getElementById("form-smb-datasource") as HTMLFormElement | null;
  const connForm = document.getElementById("form-conn-datasource") as HTMLFormElement | null;
  const smbErr = document.getElementById("smb-form-error");
  const connErr = document.getElementById("conn-form-error");
  const titleEl = document.getElementById("wizard-form-title");

  function openWizard(): void {
    smbErr?.setAttribute("hidden", "");
    connErr?.setAttribute("hidden", "");
    smbForm?.reset();
    connForm?.reset();
    syncConnPgInstallHint(null);
    showWizardPick();
    dlg?.showModal();
  }

  document.getElementById("btn-database-add")?.addEventListener("click", openWizard);
  document.getElementById("btn-wizard-close-pick")?.addEventListener("click", () => dlg?.close());

  document.getElementById("btn-wizard-back")?.addEventListener("click", () => {
    smbErr?.setAttribute("hidden", "");
    connErr?.setAttribute("hidden", "");
    smbForm?.setAttribute("hidden", "");
    connForm?.setAttribute("hidden", "");
    syncConnPgInstallHint(null);
    showWizardPick();
  });

  document.querySelectorAll<HTMLButtonElement>(".db-kind-card[data-db-kind]").forEach((btn) => {
    btn.addEventListener("click", () => {
      const kind = btn.getAttribute("data-db-kind") ?? "";
      smbErr?.setAttribute("hidden", "");
      connErr?.setAttribute("hidden", "");
      if (kind === "smb") {
        smbForm?.removeAttribute("hidden");
        connForm?.setAttribute("hidden", "");
        syncConnPgInstallHint(null);
        if (titleEl) {
          titleEl.textContent = t("database.smbDialogTitle");
        }
        showWizardForm();
        return;
      }
      smbForm?.setAttribute("hidden", "");
      connForm?.removeAttribute("hidden");
      const portEl = document.getElementById("conn-port") as HTMLInputElement | null;
      const kindInput = document.getElementById("conn-kind") as HTMLInputElement | null;
      if (kindInput) {
        kindInput.value = kind;
      }
      if (portEl) {
        portEl.value = String(DEFAULT_CONN_PORTS[kind] ?? 3306);
      }
      if (titleEl) {
        titleEl.textContent = kindShort(kind);
      }
      syncConnPgInstallHint(kind);
      showWizardForm();
    });
  });

  document.getElementById("btn-smb-cancel")?.addEventListener("click", () => dlg?.close());
  document.getElementById("btn-conn-cancel")?.addEventListener("click", () => dlg?.close());

  smbForm?.addEventListener("submit", (ev) => {
    ev.preventDefault();
    if (!smbForm || !dlg) {
      return;
    }
    const fd = new FormData(smbForm);
    const label = String(fd.get("label") ?? "").trim();
    const host = String(fd.get("host") ?? "").trim();
    const share = String(fd.get("share") ?? "").trim();
    const folder = String(fd.get("folder") ?? "").trim();
    const username = String(fd.get("username") ?? "").trim();
    const password = String(fd.get("password") ?? "");
    void (async () => {
      try {
        const created = await postDataSourceSmb({
          label,
          host,
          share,
          folder,
          username,
          password,
        });
        writeStoredSourceId(created.id);
        dlg.close();
        await loadDatabaseInfo();
      } catch (e) {
        if (smbErr) {
          smbErr.textContent = formatErr(e);
          smbErr.removeAttribute("hidden");
        }
      }
    })();
  });

  connForm?.addEventListener("submit", (ev) => {
    ev.preventDefault();
    if (!connForm || !dlg) {
      return;
    }
    const fd = new FormData(connForm);
    const kind = String(fd.get("kind") ?? "").trim().toLowerCase();
    const label = String(fd.get("label") ?? "").trim();
    const host = String(fd.get("host") ?? "").trim();
    const portRaw = String(fd.get("port") ?? "3306");
    const port = Number.parseInt(portRaw, 10);
    const database = String(fd.get("database") ?? "").trim();
    const username = String(fd.get("username") ?? "").trim();
    const password = String(fd.get("password") ?? "");
    const ssl = connForm.querySelector<HTMLInputElement>("#conn-ssl")?.checked ?? false;
    void (async () => {
      try {
        const created = await postDataSourceConnection({
          kind,
          label,
          host,
          port: Number.isFinite(port) ? port : 3306,
          database: database || undefined,
          username: username || undefined,
          password: password || undefined,
          ssl,
        });
        writeStoredSourceId(created.id);
        dlg.close();
        await loadDatabaseInfo();
      } catch (e) {
        if (connErr) {
          connErr.textContent = formatErr(e);
          connErr.removeAttribute("hidden");
        }
      }
    })();
  });
}

export function bindDatabaseTab(): void {
  bindSourceSelect();
  bindDatasourceWizard();
  document.getElementById("btn-database-refresh")?.addEventListener("click", () => {
    void loadDatabaseInfo();
  });
  document.getElementById("tab-database")?.addEventListener("click", () => {
    void loadDatabaseInfo();
  });
}
