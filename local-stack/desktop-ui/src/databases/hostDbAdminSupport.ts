/**
 * Engines for which deploy-control implements privileged admin DDL
 * (`/api/v2/host-databases/.../admin/create-database|create-table`).
 * Extend when adding backend support (see `db_admin.rs` / `service.rs`).
 */
const ADMIN_CREATE_ENGINES = new Set<string>(["postgresql", "mysql"]);

export function hostDbAdminCreateSupported(engine: string): boolean {
  return ADMIN_CREATE_ENGINES.has(engine);
}
