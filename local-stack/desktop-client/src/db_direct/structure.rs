//! Table structure: columns, constraints (PG first).

use serde::Serialize;
use sqlx::PgPool;

use super::QueryResultView;
use super::pg_ops::pg_run_readonly_sql;

fn ident_ok(s: &str) -> bool {
    !s.is_empty() && s.chars().all(|c| c.is_ascii_alphanumeric() || c == '_')
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct PgStructureView {
    pub columns: Option<QueryResultView>,
    pub foreign_keys: Option<QueryResultView>,
}

pub async fn pg_table_structure(
    pool: &PgPool,
    schema: &str,
    table: &str,
) -> Result<PgStructureView, String> {
    if !ident_ok(schema) || !ident_ok(table) {
        return Err("invalid schema or table name".into());
    }
    let q_cols = format!(
        "SELECT column_name, data_type, is_nullable, column_default, ordinal_position
         FROM information_schema.columns
         WHERE table_schema = '{schema}' AND table_name = '{table}'
         ORDER BY ordinal_position"
    );
    let q_fk = format!(
        "SELECT
            tc.constraint_name,
            kcu.column_name,
            ccu.table_schema AS foreign_table_schema,
            ccu.table_name AS foreign_table_name,
            ccu.column_name AS foreign_column_name
         FROM information_schema.table_constraints AS tc
         JOIN information_schema.key_column_usage AS kcu
           ON tc.constraint_name = kcu.constraint_name AND tc.table_schema = kcu.table_schema
         JOIN information_schema.constraint_column_usage AS ccu
           ON ccu.constraint_name = tc.constraint_name AND ccu.table_schema = tc.table_schema
         WHERE tc.constraint_type = 'FOREIGN KEY'
           AND tc.table_schema = '{schema}'
           AND tc.table_name = '{table}'"
    );
    let columns = pg_run_readonly_sql(pool, &q_cols, 2000).await.ok();
    let foreign_keys = pg_run_readonly_sql(pool, &q_fk, 500).await.ok();
    Ok(PgStructureView {
        columns,
        foreign_keys,
    })
}
