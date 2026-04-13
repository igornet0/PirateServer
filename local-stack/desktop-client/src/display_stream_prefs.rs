//! Stored preferences for display streaming (receive / send) in `connection` row.

use rusqlite::Connection;

use crate::desktop_store;

pub fn get_prefs() -> (bool, bool) {
    let Ok(c) = desktop_store::open() else {
        return (false, false);
    };
    read_prefs(&c).unwrap_or((false, false))
}

fn read_prefs(c: &Connection) -> Result<(bool, bool), rusqlite::Error> {
    let mut stmt = c.prepare(
        "SELECT display_stream_allow_receive, display_stream_allow_send FROM connection WHERE id = 1",
    )?;
    stmt.query_row([], |row| {
        let r: i64 = row.get(0)?;
        let s: i64 = row.get(1)?;
        Ok((r != 0, s != 0))
    })
}

pub fn set_prefs(allow_receive: bool, allow_send: bool) -> Result<(), String> {
    let c = desktop_store::open().map_err(|e| e.to_string())?;
    let r = if allow_receive { 1i64 } else { 0 };
    let s = if allow_send { 1i64 } else { 0 };
    c.execute(
        "UPDATE connection SET display_stream_allow_receive = ?1, display_stream_allow_send = ?2 WHERE id = 1",
        rusqlite::params![r, s],
    )
    .map_err(|e| e.to_string())?;
    Ok(())
}
