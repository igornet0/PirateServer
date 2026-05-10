import { invoke } from "@tauri-apps/api/core";

export type DbCredsInvoke = () => Record<string, string | null | undefined>;

export type HostQueryResult = {
  columns: string[];
  rows: Array<Record<string, unknown> | string | number | boolean | null>;
  row_count: number;
  truncated?: boolean;
  warn?: string;
};

export function parseSchemaNames(json: string): string[] {
  const j = JSON.parse(json) as { schemas?: Array<{ name?: string } | string> };
  const arr = j.schemas;
  if (!Array.isArray(arr)) return [];
  return arr
    .map((x) => (typeof x === "string" ? x : (x as { name?: string }).name))
    .filter((x): x is string => typeof x === "string" && x.length > 0);
}

export function parseTableNames(json: string): { name: string; schema_name?: string }[] {
  const j = JSON.parse(json) as { tables?: Array<{ name?: string; schema_name?: string }> };
  const arr = j.tables;
  if (!Array.isArray(arr)) return [];
  return arr
    .map((t) => ({ name: t.name ?? "", schema_name: t.schema_name }))
    .filter((t) => t.name.length > 0);
}

export function parseColumns(json: string): { name: string; type: string }[] {
  const j = JSON.parse(json) as { columns?: Array<Record<string, string | null | undefined>> };
  const arr = j.columns;
  if (!Array.isArray(arr)) return [];
  return arr.map((c) => ({
    name: String(c.column_name ?? c.name ?? "—"),
    type: String(c.data_type ?? c.type ?? "—"),
  }));
}

export function extractTableRows(previewResponse: unknown): unknown[] {
  if (!previewResponse || typeof previewResponse !== "object") return [];
  const j = previewResponse as { preview?: { rows?: unknown } };
  const p = j.preview;
  if (!p) return [];
  const r = p.rows;
  if (Array.isArray(r)) return r;
  return [];
}

export function rowKeysForGrid(rows: unknown[]): string[] {
  const keys = new Set<string>();
  for (const r of rows) {
    if (r && typeof r === "object" && !Array.isArray(r)) {
      for (const k of Object.keys(r as object)) {
        keys.add(k);
      }
    }
  }
  return Array.from(keys);
}

export async function hostDbSchemas(instanceId: string, creds: DbCredsInvoke): Promise<string[]> {
  const json = await invoke<string>("control_api_host_db_schemas_json", {
    instanceId,
    ...creds(),
  });
  return parseSchemaNames(json);
}

export async function hostDbTables(
  instanceId: string,
  schema: string,
  creds: DbCredsInvoke,
): Promise<{ name: string; schema_name?: string }[]> {
  const json = await invoke<string>("control_api_host_db_tables_json", {
    instanceId,
    schema,
    ...creds(),
  });
  return parseTableNames(json);
}

export async function hostDbColumns(
  instanceId: string,
  schema: string,
  table: string,
  creds: DbCredsInvoke,
): Promise<{ name: string; type: string }[]> {
  const json = await invoke<string>("control_api_host_db_columns_json", {
    instanceId,
    schema,
    table,
    ...creds(),
  });
  return parseColumns(json);
}

export async function hostDbRows(
  instanceId: string,
  schema: string,
  table: string,
  limit: number,
  offset: number,
  creds: DbCredsInvoke,
): Promise<{ rows: unknown[]; parsed: unknown }> {
  const rj = await invoke<string>("control_api_host_db_rows_json", {
    instanceId,
    schema,
    table,
    limit,
    offset,
    ...creds(),
  });
  const parsed = JSON.parse(rj) as unknown;
  return { rows: extractTableRows(parsed), parsed };
}

export async function hostDbQuery(
  instanceId: string,
  sql: string,
  maxRows: number,
  creds: DbCredsInvoke,
): Promise<HostQueryResult> {
  const json = await invoke<string>("control_api_host_db_query_json", {
    instanceId,
    sql,
    maxRows,
    database: null,
    ...creds(),
  });
  return JSON.parse(json) as HostQueryResult;
}

export async function hostDbRelationshipsJson(instanceId: string, creds: DbCredsInvoke): Promise<string> {
  return invoke<string>("control_api_host_db_relationships_json", {
    instanceId,
    ...creds(),
  });
}
