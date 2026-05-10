//! Host databases workspace v2: metadata tree, data grid, optional writes, async SQL jobs.
//! Feature flags: `CONTROL_API_HOST_DB_WORKSPACE_V2`, `CONTROL_API_HOST_DB_WRITE`, `CONTROL_API_HOST_DB_SQL_JOBS`,
//! `CONTROL_API_HOST_DB_MIGRATIONS`, `CONTROL_API_HOST_DB_ADMIN_CREATE`, `CONTROL_API_HOST_DB_MIGRATION_RUN`.
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;

use axum::extract::{Path, Query, State};
use axum::Json;
use deploy_control::{
    HostDatabaseQueryBody, HostDatabaseQueryResultView,     HostDbAdminCreateTableBody, HostDbAdminCreateUserBody,
    HostDbAdminCreateUserView, HostDbAdminDeleteUserBody, HostDbAdminDeleteUserView, HostDbCreateDatabaseBody,
    HostDbMigrationRunBody, HostDbMigrationRunView,
    HostDbMigrationStatusView, HostDbV2CapabilitiesView, HostDbV2GridBody, HostDbV2GridView,
    HostDbV2MigrationStatusBody, HostDbV2MutationResultView, HostDbV2ObjectTreeView, HostDbV2RowMutationBody,
    HostDbV2SqlJobStartBody, HostDbV2SqlJobView, HostDbRequestCredentials,
};
use serde::Deserialize;
use uuid::Uuid;

use crate::error::ApiError;
use crate::host_databases_api::{host_db_creds_from_headers, sql_fingerprint_for_audit};
use crate::ApiState;
use axum::http::HeaderMap;

const MAX_SQL_JOBS: usize = 64;

/// In-flight row for [`ApiState::host_db_sql_jobs`].
pub(crate) struct HostDbSqlJobRec {
    pub(crate) status: String,
    pub(crate) result: Option<HostDatabaseQueryResultView>,
    pub(crate) error: Option<String>,
    pub(crate) cancel: Arc<AtomicBool>,
}

impl Clone for HostDbSqlJobRec {
    fn clone(&self) -> Self {
        Self {
            status: self.status.clone(),
            result: self.result.clone(),
            error: self.error.clone(),
            cancel: Arc::clone(&self.cancel),
        }
    }
}

fn check_host_databases_v2(s: &ApiState) -> Result<(), ApiError> {
    if !s.host_databases_enabled {
        return Err(ApiError::forbidden(
            "host database API is disabled (set CONTROL_API_HOST_DATABASES=1 to enable)",
        ));
    }
    if !s.host_db_workspace_v2 {
        return Err(ApiError::forbidden(
            "host DB workspace v2 is disabled (set CONTROL_API_HOST_DB_WORKSPACE_V2=1 to enable)",
        ));
    }
    Ok(())
}

fn check_host_db_migrations(s: &ApiState) -> Result<(), ApiError> {
    check_host_databases_v2(s)?;
    if !s.host_db_migrations_enabled {
        return Err(ApiError::forbidden(
            "migration status API is disabled (set CONTROL_API_HOST_DB_MIGRATIONS=1)",
        ));
    }
    Ok(())
}

fn check_host_db_admin_create(s: &ApiState) -> Result<(), ApiError> {
    check_host_databases_v2(s)?;
    if !s.host_db_admin_create_enabled {
        return Err(ApiError::forbidden(
            "admin create-database is disabled (set CONTROL_API_HOST_DB_ADMIN_CREATE=1)",
        ));
    }
    Ok(())
}

fn check_host_db_migration_run(s: &ApiState) -> Result<(), ApiError> {
    check_host_databases_v2(s)?;
    if !s.host_db_migration_run_enabled {
        return Err(ApiError::forbidden(
            "migration run is disabled (set CONTROL_API_HOST_DB_MIGRATION_RUN=1)",
        ));
    }
    Ok(())
}

#[derive(Debug, Deserialize)]
pub struct HostDbMigrationStatusQuery {
    pub database: String,
    /// Optional comma-separated: `alembic,prisma,flyway`
    #[serde(default)]
    pub tools: Option<String>,
}

fn filter_migration_status_by_tools(
    mut view: HostDbMigrationStatusView,
    tools: Option<&str>,
) -> HostDbMigrationStatusView {
    let Some(raw) = tools else {
        return view;
    };
    let want: std::collections::HashSet<String> = raw
        .split(',')
        .map(|t| t.trim().to_lowercase())
        .filter(|t| !t.is_empty())
        .collect();
    if want.is_empty() {
        return view;
    }
    view.tools.retain(|t| want.contains(&t.tool.to_lowercase()));
    view
}

/// `GET /api/v2/host-databases/capabilities`
pub async fn api_host_db_v2_capabilities(
    State(s): State<ApiState>,
    headers: HeaderMap,
) -> Result<Json<HostDbV2CapabilitiesView>, ApiError> {
    crate::check_api_bearer(&s, &headers)?;
    check_host_databases_v2(&s)?;
    Ok(Json(HostDbV2CapabilitiesView {
        workspace_v2: s.host_db_workspace_v2,
        write: s.host_db_write_enabled,
        sql_jobs: s.host_db_sql_jobs_enabled,
        migration_status: s.host_db_migrations_enabled,
        admin_create_database: s.host_db_admin_create_enabled,
        admin_create_table: s.host_db_admin_create_enabled,
        admin_create_user: s.host_db_admin_create_enabled,
        migration_run: s.host_db_migration_run_enabled,
    }))
}

/// `GET /api/v2/host-databases/:instance_id/object-tree`
pub async fn api_host_db_v2_object_tree(
    State(s): State<ApiState>,
    headers: HeaderMap,
    Path(instance_id): Path<String>,
) -> Result<Json<HostDbV2ObjectTreeView>, ApiError> {
    crate::check_api_bearer(&s, &headers)?;
    check_host_databases_v2(&s)?;
    let creds = host_db_creds_from_headers(&headers);
    s.plane
        .host_db_v2_object_tree(&instance_id, creds.as_ref())
        .await
        .map(Json)
        .map_err(Into::into)
}

/// `POST /api/v2/host-databases/:instance_id/grid`
pub async fn api_host_db_v2_grid(
    State(s): State<ApiState>,
    headers: HeaderMap,
    Path(instance_id): Path<String>,
    Json(body): Json<HostDbV2GridBody>,
) -> Result<Json<HostDbV2GridView>, ApiError> {
    crate::check_api_bearer(&s, &headers)?;
    check_host_databases_v2(&s)?;
    if body.schema.len() > 256 || body.table.len() > 256 {
        return Err(ApiError::bad_request("schema/table too long"));
    }
    let creds = host_db_creds_from_headers(&headers);
    s.plane
        .host_db_v2_grid(&instance_id, &body, creds.as_ref())
        .await
        .map(Json)
        .map_err(Into::into)
}

/// `POST /api/v2/host-databases/:instance_id/row-mutate` (write when enabled)
pub async fn api_host_db_v2_row_mutate(
    State(s): State<ApiState>,
    headers: HeaderMap,
    Path(instance_id): Path<String>,
    Json(body): Json<HostDbV2RowMutationBody>,
) -> Result<Json<HostDbV2MutationResultView>, ApiError> {
    crate::check_api_bearer(&s, &headers)?;
    check_host_databases_v2(&s)?;
    if !s.host_db_write_enabled {
        return Err(ApiError::forbidden(
            "row mutations are disabled (set CONTROL_API_HOST_DB_WRITE=1 on control-api)",
        ));
    }
    let creds = host_db_creds_from_headers(&headers);
    s.plane
        .host_db_v2_row_mutate(
            &instance_id,
            &body,
            creds.as_ref(),
            s.host_db_write_enabled,
        )
        .await
        .map(Json)
        .map_err(Into::into)
}

/// `POST /api/v2/host-databases/:instance_id/sql-jobs` — start async read-only query job.
pub async fn api_host_db_v2_sql_job_start(
    State(s): State<ApiState>,
    headers: HeaderMap,
    Path(instance_id): Path<String>,
    Json(mut body): Json<HostDbV2SqlJobStartBody>,
) -> Result<Json<HostDbV2SqlJobView>, ApiError> {
    crate::check_api_bearer(&s, &headers)?;
    check_host_databases_v2(&s)?;
    if !s.host_db_sql_jobs_enabled {
        return Err(ApiError::forbidden(
            "SQL jobs are disabled (set CONTROL_API_HOST_DB_SQL_JOBS=1 to enable)",
        ));
    }
    if body.sql.len() > s.host_db_max_sql_bytes {
        return Err(ApiError::bad_request(format!(
            "SQL exceeds max length ({} > {}); raise CONTROL_API_HOST_DB_MAX_SQL_BYTES if needed",
            body.sql.len(),
            s.host_db_max_sql_bytes
        )));
    }
    if body.max_rows == 0 {
        body.max_rows = 500;
    }
    body.max_rows = body.max_rows.clamp(1, s.host_db_max_query_rows);
    let fp = sql_fingerprint_for_audit(&body.sql);
    tracing::info!(
        target: "pirate.db.audit",
        op = "sql_job",
        instance = %instance_id,
        sql_len = body.sql.len(),
        max_rows = body.max_rows,
        sql_fingerprint = %fp,
        "v2 host database sql job (fingerprint only)"
    );

    let creds_owned: Option<HostDbRequestCredentials> = host_db_creds_from_headers(&headers).map(|c| {
        HostDbRequestCredentials {
            user: c.user,
            pass: c.pass,
        }
    });
    {
        let m = s.host_db_sql_jobs.lock().unwrap();
        if m.len() >= MAX_SQL_JOBS {
            return Err(ApiError::service_unavailable(
                "too many in-flight host DB SQL jobs; try again later",
            ));
        }
    }

    let job_id = Uuid::new_v4().to_string();
    let cancel = Arc::new(AtomicBool::new(false));
    {
        let mut m = s.host_db_sql_jobs.lock().unwrap();
        m.insert(
            job_id.clone(),
            HostDbSqlJobRec {
                status: "queued".into(),
                result: None,
                error: None,
                cancel: Arc::clone(&cancel),
            },
        );
    }

    let plane = s.plane.clone();
    let jobs = s.host_db_sql_jobs.clone();
    let job_id2 = job_id.clone();
    let inst2 = instance_id.clone();
    let sql = body.sql.clone();
    let max_rows = body.max_rows;
    let creds2 = creds_owned;

    tokio::spawn(async move {
        {
            let mut m = jobs.lock().unwrap();
            if let Some(r) = m.get_mut(&job_id2) {
                r.status = "running".into();
            }
        }
        if let Some(r) = jobs.lock().unwrap().get(&job_id2) {
            if r.cancel.load(Ordering::Relaxed) {
                let mut m = jobs.lock().unwrap();
                if let Some(rec) = m.get_mut(&job_id2) {
                    rec.status = "cancelled".into();
                    rec.error = Some("cancelled".into());
                }
                return;
            }
        }
        let res = plane
            .host_db_query(
                &inst2,
                &HostDatabaseQueryBody {
                    sql,
                    max_rows,
                    database: None,
                },
                creds2.as_ref(),
            )
            .await;
        let mut m = jobs.lock().unwrap();
        let Some(rec) = m.get_mut(&job_id2) else {
            return;
        };
        match res {
            Ok(v) => {
                rec.status = "done".into();
                rec.result = Some(v);
            }
            Err(e) => {
                rec.status = "error".into();
                rec.error = Some(e.to_string());
            }
        }
    });

    Ok(Json(HostDbV2SqlJobView {
        job_id: job_id.clone(),
        status: "queued".into(),
        result: None,
        error: None,
    }))
}

/// `GET /api/v2/host-databases/:instance_id/sql-jobs/:job_id`
pub async fn api_host_db_v2_sql_job_get(
    State(s): State<ApiState>,
    headers: HeaderMap,
    Path((instance_id, job_id)): Path<(String, String)>,
) -> Result<Json<HostDbV2SqlJobView>, ApiError> {
    crate::check_api_bearer(&s, &headers)?;
    check_host_databases_v2(&s)?;
    if !s.host_db_sql_jobs_enabled {
        return Err(ApiError::forbidden(
            "SQL jobs are disabled (set CONTROL_API_HOST_DB_SQL_JOBS=1 to enable)",
        ));
    }
    let _ = instance_id; // instance is validated when job is created; jobs are not partitioned in MVP
    let m = s.host_db_sql_jobs.lock().unwrap();
    let Some(r) = m.get(&job_id) else {
        return Err(ApiError::bad_request("unknown sql job_id"));
    };
    Ok(Json(HostDbV2SqlJobView {
        job_id: job_id.clone(),
        status: r.status.clone(),
        result: r.result.clone(),
        error: r.error.clone(),
    }))
}

/// `DELETE /api/v2/host-databases/:instance_id/sql-jobs/:job_id` — best-effort cancel
pub async fn api_host_db_v2_sql_job_cancel(
    State(s): State<ApiState>,
    headers: HeaderMap,
    Path((instance_id, job_id)): Path<(String, String)>,
) -> Result<Json<HostDbV2SqlJobView>, ApiError> {
    crate::check_api_bearer(&s, &headers)?;
    check_host_databases_v2(&s)?;
    if !s.host_db_sql_jobs_enabled {
        return Err(ApiError::forbidden(
            "SQL jobs are disabled (set CONTROL_API_HOST_DB_SQL_JOBS=1 to enable)",
        ));
    }
    let _ = instance_id;
    let m = s.host_db_sql_jobs.lock().unwrap();
    let Some(r) = m.get(&job_id) else {
        return Err(ApiError::bad_request("unknown sql job_id"));
    };
    r.cancel.store(true, Ordering::Relaxed);
    Ok(Json(HostDbV2SqlJobView {
        job_id: job_id.clone(),
        status: "cancelling".into(),
        result: r.result.clone(),
        error: r.error.clone(),
    }))
}

/// `GET /api/v2/host-databases/:instance_id/migration-status?database=...`
pub async fn api_host_db_v2_migration_status_get(
    State(s): State<ApiState>,
    headers: HeaderMap,
    Path(instance_id): Path<String>,
    Query(q): Query<HostDbMigrationStatusQuery>,
) -> Result<Json<HostDbMigrationStatusView>, ApiError> {
    crate::check_api_bearer(&s, &headers)?;
    check_host_db_migrations(&s)?;
    if q.database.is_empty() || q.database.len() > 200 {
        return Err(ApiError::bad_request("invalid database query parameter"));
    }
    let creds = host_db_creds_from_headers(&headers);
    tracing::info!(
        target: "pirate.db.audit",
        op = "migration_status",
        instance = %instance_id,
        database = %q.database,
        "v2 host database migration status (read-only)"
    );
    s.plane
        .host_db_migration_status(&instance_id, &q.database, creds.as_ref())
        .await
        .map(|v| Json(filter_migration_status_by_tools(v, q.tools.as_deref())))
        .map_err(Into::into)
}

/// `POST /api/v2/host-databases/:instance_id/migration-status` (same as GET with `database` in JSON body).
pub async fn api_host_db_v2_migration_status_post(
    State(s): State<ApiState>,
    headers: HeaderMap,
    Path(instance_id): Path<String>,
    Json(body): Json<HostDbV2MigrationStatusBody>,
) -> Result<Json<HostDbMigrationStatusView>, ApiError> {
    crate::check_api_bearer(&s, &headers)?;
    check_host_db_migrations(&s)?;
    if body.database.is_empty() || body.database.len() > 200 {
        return Err(ApiError::bad_request("invalid database in body"));
    }
    let creds = host_db_creds_from_headers(&headers);
    tracing::info!(
        target: "pirate.db.audit",
        op = "migration_status",
        instance = %instance_id,
        database = %body.database,
        "v2 host database migration status (read-only, POST body)"
    );
    s.plane
        .host_db_migration_status(&instance_id, &body.database, creds.as_ref())
        .await
        .map(|v| Json(filter_migration_status_by_tools(v, body.tools.as_deref())))
        .map_err(Into::into)
}

/// `POST /api/v2/host-databases/:instance_id/admin/create-database`
pub async fn api_host_db_v2_admin_create_database(
    State(s): State<ApiState>,
    headers: HeaderMap,
    Path(instance_id): Path<String>,
    Json(body): Json<HostDbCreateDatabaseBody>,
) -> Result<Json<serde_json::Value>, ApiError> {
    crate::check_api_bearer(&s, &headers)?;
    check_host_db_admin_create(&s)?;
    if body.database.is_empty() || body.database.len() > 200 {
        return Err(ApiError::bad_request("invalid database name"));
    }
    tracing::info!(
        target: "pirate.db.audit",
        op = "admin_create_database",
        instance = %instance_id,
        database = %body.database,
        owner = ?body.owner,
        encoding = ?body.encoding,
        "v2 host database admin create (privileged)"
    );
    let creds = host_db_creds_from_headers(&headers);
    s.plane
        .host_db_admin_create_database(&instance_id, &body, creds.as_ref())
        .await
        .map(|_| Json(serde_json::json!({ "ok": true })))
        .map_err(Into::into)
}

/// `POST /api/v2/host-databases/:instance_id/admin/create-table`
pub async fn api_host_db_v2_admin_create_table(
    State(s): State<ApiState>,
    headers: HeaderMap,
    Path(instance_id): Path<String>,
    Json(body): Json<HostDbAdminCreateTableBody>,
) -> Result<Json<serde_json::Value>, ApiError> {
    crate::check_api_bearer(&s, &headers)?;
    check_host_db_admin_create(&s)?;
    if body.database.is_empty() || body.table.is_empty() || body.schema.is_empty() {
        return Err(ApiError::bad_request("invalid database, schema, or table"));
    }
    if body.columns.is_empty() {
        return Err(ApiError::bad_request("columns must be non-empty"));
    }
    tracing::info!(
        target: "pirate.db.audit",
        op = "admin_create_table",
        instance = %instance_id,
        database = %body.database,
        schema = %body.schema,
        table = %body.table,
        if_not_exists = body.if_not_exists,
        column_count = body.columns.len(),
        "v2 host database admin create table (DDL)"
    );
    s.plane
        .host_db_admin_create_table(&instance_id, &body)
        .await
        .map(|_| Json(serde_json::json!({ "ok": true })))
        .map_err(Into::into)
}

/// `POST /api/v2/host-databases/:instance_id/admin/create-user` — login role; password never logged.
pub async fn api_host_db_v2_admin_create_user(
    State(s): State<ApiState>,
    headers: HeaderMap,
    Path(instance_id): Path<String>,
    Json(body): Json<HostDbAdminCreateUserBody>,
) -> Result<Json<HostDbAdminCreateUserView>, ApiError> {
    crate::check_api_bearer(&s, &headers)?;
    check_host_db_admin_create(&s)?;
    if body.database.is_empty() || body.username.is_empty() {
        return Err(ApiError::bad_request("invalid database or username"));
    }
    tracing::info!(
        target: "pirate.db.audit",
        op = "admin_create_user",
        instance = %instance_id,
        database = %body.database,
        username = %body.username,
        generate_password = body.generate_password,
        privileges = %body.privileges,
        allow_schema_ddl = body.allow_schema_ddl,
        "v2 host database admin create user (login role; password not logged)"
    );
    let creds = host_db_creds_from_headers(&headers).ok_or_else(|| {
        ApiError::bad_request(
            "database credentials are required: set X-Pirate-Db-User and X-Pirate-Db-Password (same as for host database browse)",
        )
    })?;
    s.plane
        .host_db_admin_create_user(&instance_id, &body, &creds)
        .await
        .map(Json)
        .map_err(Into::into)
}

/// `POST /api/v2/host-databases/:instance_id/admin/delete-user` — drop login role (PostgreSQL).
pub async fn api_host_db_v2_admin_delete_user(
    State(s): State<ApiState>,
    headers: HeaderMap,
    Path(instance_id): Path<String>,
    Json(body): Json<HostDbAdminDeleteUserBody>,
) -> Result<Json<HostDbAdminDeleteUserView>, ApiError> {
    crate::check_api_bearer(&s, &headers)?;
    check_host_db_admin_create(&s)?;
    if body.username.is_empty() || body.username.len() > 200 {
        return Err(ApiError::bad_request("invalid username"));
    }
    tracing::info!(
        target: "pirate.db.audit",
        op = "admin_delete_user",
        instance = %instance_id,
        username = %body.username,
        drop_owned_all_databases = body.drop_owned_all_databases,
        "v2 host database admin delete user (role)"
    );
    let creds = host_db_creds_from_headers(&headers).ok_or_else(|| {
        ApiError::bad_request(
            "database credentials are required: set X-Pirate-Db-User and X-Pirate-Db-Password (same as for host database browse)",
        )
    })?;
    s.plane
        .host_db_admin_delete_user(&instance_id, &body, &creds)
        .await
        .map(Json)
        .map_err(Into::into)
}

/// `POST /api/v2/host-databases/:instance_id/migration-run`
pub async fn api_host_db_v2_migration_run(
    State(s): State<ApiState>,
    headers: HeaderMap,
    Path(instance_id): Path<String>,
    Json(body): Json<HostDbMigrationRunBody>,
) -> Result<Json<HostDbMigrationRunView>, ApiError> {
    crate::check_api_bearer(&s, &headers)?;
    check_host_db_migration_run(&s)?;
    if body.workdir.is_empty() || body.workdir.len() > 4096 {
        return Err(ApiError::bad_request("invalid workdir"));
    }
    if body.tool.is_empty() || body.tool.len() > 32 {
        return Err(ApiError::bad_request("invalid tool"));
    }
    tracing::info!(
        target: "pirate.db.audit",
        op = "migration_run",
        instance = %instance_id,
        tool = %body.tool,
        workdir = %body.workdir,
        "v2 host migration run (whitelisted command)"
    );
    s.plane
        .host_db_migration_run(&instance_id, &body)
        .await
        .map(Json)
        .map_err(Into::into)
}

#[cfg(test)]
mod tests {
    use crate::host_databases_api::sql_fingerprint_for_audit;

    #[test]
    fn v2_fingerprint() {
        let a = sql_fingerprint_for_audit("SELECT 1");
        assert!(!a.is_empty());
    }
}
