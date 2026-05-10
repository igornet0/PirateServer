//! Read-only SQL policy (aligned with `deploy-control` `db_host::is_readonly_sql`).

#[derive(Debug, Clone)]
pub struct ReadonlySqlError(pub String);

impl std::fmt::Display for ReadonlySqlError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", self.0)
    }
}

impl std::error::Error for ReadonlySqlError {}

/// Returns `Ok(())` if the statement is allowed for ad-hoc explorer queries.
pub fn is_readonly_sql(sql: &str) -> Result<(), ReadonlySqlError> {
    let t = sql.trim();
    if t.is_empty() {
        return Err(ReadonlySqlError("empty SQL".into()));
    }
    if t.contains(';') {
        let ttrim = t.trim_end();
        if ttrim.contains(';') {
            let without_trailing = ttrim.trim_end_matches(';');
            if without_trailing.contains(';') {
                return Err(ReadonlySqlError(
                    "multiple statements are not allowed".into(),
                ));
            }
        }
    }
    let up = t.to_uppercase();
    let first = up.split_whitespace().next().unwrap_or("");
    if matches!(
        first,
        "SELECT" | "SHOW" | "EXPLAIN" | "DESCRIBE" | "DESC" | "WITH" | "HELP" | "TABLES"
    ) {
        return Ok(());
    }
    if matches!(
        first,
        "INSERT" | "UPDATE" | "DELETE" | "MERGE" | "REPLACE" | "ALTER" | "DROP" | "CREATE"
            | "TRUNCATE" | "GRANT" | "REVOKE" | "CALL" | "DO" | "SET" | "USE" | "START"
            | "BEGIN" | "COMMIT" | "ROLLBACK" | "LOCK" | "UNLOCK" | "LOAD" | "COPY"
    ) {
        return Err(ReadonlySqlError(format!(
            "write / DDL / session control not allowed: {first}"
        )));
    }
    Err(ReadonlySqlError(format!(
        "only read-only SQL is allowed (got: {first})"
    )))
}

#[cfg(test)]
mod tests {
    use super::is_readonly_sql;

    #[test]
    fn select_ok() {
        assert!(is_readonly_sql("SELECT 1").is_ok());
    }

    #[test]
    fn insert_rejected() {
        assert!(is_readonly_sql("INSERT INTO t VALUES (1)").is_err());
    }
}
