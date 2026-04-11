//! Single SQLite database for gRPC connection, bookmarks, and monitoring samples.

use rusqlite::Connection;
use std::path::PathBuf;

fn migrate_bookmarks(c: &Connection) -> Result<(), rusqlite::Error> {
    let mut stmt = c.prepare("PRAGMA table_info(bookmarks)")?;
    let cols: Vec<String> = stmt
        .query_map([], |row| row.get::<_, String>(1))?
        .filter_map(|r| r.ok())
        .collect();
    if !cols.iter().any(|n| n == "server_pubkey_b64") {
        c.execute(
            "ALTER TABLE bookmarks ADD COLUMN server_pubkey_b64 TEXT",
            [],
        )?;
    }
    if !cols.iter().any(|n| n == "paired") {
        c.execute(
            "ALTER TABLE bookmarks ADD COLUMN paired INTEGER NOT NULL DEFAULT 0",
            [],
        )?;
    }
    Ok(())
}

pub fn db_path() -> PathBuf {
    dirs::data_local_dir()
        .unwrap_or_else(|| PathBuf::from("."))
        .join("PirateClient")
        .join("pirate_desktop.db")
}

pub fn open() -> Result<Connection, rusqlite::Error> {
    let path = db_path();
    if let Some(parent) = path.parent() {
        let _ = std::fs::create_dir_all(parent);
    }
    let c = Connection::open(path)?;
    c.execute_batch(
        "CREATE TABLE IF NOT EXISTS connection (
            id INTEGER PRIMARY KEY CHECK (id = 1),
            url TEXT NOT NULL DEFAULT '',
            server_pubkey_b64 TEXT,
            paired INTEGER NOT NULL DEFAULT 0,
            project_id TEXT NOT NULL DEFAULT 'default'
        );
        CREATE TABLE IF NOT EXISTS bookmarks (
            id TEXT PRIMARY KEY,
            label TEXT NOT NULL,
            url TEXT NOT NULL UNIQUE,
            server_pubkey_b64 TEXT,
            paired INTEGER NOT NULL DEFAULT 0
        );
        CREATE TABLE IF NOT EXISTS samples (
            ts_ms INTEGER PRIMARY KEY,
            cpu REAL NOT NULL,
            mem_used INTEGER NOT NULL
        );
        INSERT OR IGNORE INTO connection (id, url, paired, project_id) VALUES (1, '', 0, 'default');",
    )?;
    migrate_bookmarks(&c)?;
    migrate_connection_control_api(&c)?;
    Ok(c)
}

fn migrate_connection_control_api(c: &Connection) -> Result<(), rusqlite::Error> {
    let mut stmt = c.prepare("PRAGMA table_info(connection)")?;
    let cols: Vec<String> = stmt
        .query_map([], |row| row.get::<_, String>(1))?
        .filter_map(|r| r.ok())
        .collect();
    if !cols.iter().any(|n| n == "control_api_base_url") {
        c.execute(
            "ALTER TABLE connection ADD COLUMN control_api_base_url TEXT NOT NULL DEFAULT ''",
            [],
        )?;
    }
    Ok(())
}
