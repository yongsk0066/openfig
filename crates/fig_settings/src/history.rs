use std::time::SystemTime;

use inner::Inner;
use parking_lot::MutexGuard;
pub use rusqlite;
use rusqlite::hooks::{
    AuthAction,
    AuthContext,
    Authorization,
};
use rusqlite::types::ValueRef;
use rusqlite::{
    Connection,
    params,
};
use serde_json::Value;
use tracing::trace;

use crate::sqlite::{
    Db,
    database,
};
use crate::{
    Error,
    Result,
};

type Authorizer = fn(AuthContext<'_>) -> Authorization;

/// Allows a statement to read the `history` table and nothing else.
///
/// `Read` is emitted per column with the resolved table name, so a query that reaches `state` or
/// `auth_kv` — directly, through a view, a CTE or a subquery — is refused at prepare time. Every
/// write action, `PRAGMA`, `ATTACH` and DDL action falls through to `Deny`. Scalar and aggregate
/// functions are allowed: they cannot reach a table that the `Read` rule has not already
/// authorised.
fn history_reads_only(ctx: AuthContext<'_>) -> Authorization {
    match ctx.action {
        AuthAction::Select | AuthAction::Recursive | AuthAction::Function { .. } => Authorization::Allow,
        AuthAction::Read {
            table_name: "history", ..
        } => Authorization::Allow,
        _ => Authorization::Deny,
    }
}

const ALL_COLUMNS: &str = "id, command, shell, pid, session_id, cwd, start_time, duration, hostname, exit_code";

#[derive(Debug, Clone, Default)]
pub struct CommandInfo {
    pub command: Option<String>,
    pub shell: Option<String>,
    pub pid: Option<i32>,
    pub session_id: Option<String>,
    pub cwd: Option<String>,
    pub start_time: Option<SystemTime>,
    pub end_time: Option<SystemTime>,
    pub hostname: Option<String>,
    pub exit_code: Option<i32>,
}

#[derive(Debug, Default)]
pub struct History(inner::Inner);

mod inner {
    use crate::sqlite::Db;

    #[derive(Debug, Default)]
    pub enum Inner {
        #[default]
        Global,
        Owned(Db),
    }
}

impl History {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn mock() -> Self {
        let db = Db::mock();
        db.migrate().expect("Failed to migrate database");
        Self(inner::Inner::Owned(db))
    }

    fn db(&self) -> Result<&Db> {
        match &self.0 {
            Inner::Owned(db) => Ok(db),
            Inner::Global => Ok(database()?),
        }
    }

    fn conn(&self) -> Result<MutexGuard<'_, Connection>> {
        Ok(self.db()?.conn())
    }

    pub fn insert_command_history(&self, command_info: &CommandInfo) -> Result<()> {
        trace!("Inserting command into history: {:?}", command_info);
        // Insert the command into the history table
        // Ensure that the command is not empty
        if let Some(command) = &command_info.command
            && !command.is_empty()
        {
            self.conn()?.execute(
                "INSERT INTO history 
                        (command, shell, pid, session_id, cwd, start_time, end_time, duration, hostname, exit_code)
                        VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?)",
                params![
                    &command_info.command,
                    &command_info.shell,
                    &command_info.pid,
                    &command_info.session_id,
                    &command_info.cwd,
                    &command_info
                        .start_time
                        .as_ref()
                        .and_then(|t| t.duration_since(std::time::UNIX_EPOCH).ok())
                        .and_then(|d| i64::try_from(d.as_secs()).ok()),
                    &command_info
                        .end_time
                        .as_ref()
                        .and_then(|t| t.duration_since(std::time::UNIX_EPOCH).ok())
                        .and_then(|t| i64::try_from(t.as_secs()).ok()),
                    &command_info
                        .start_time
                        .as_ref()
                        .and_then(|start_time| {
                            command_info
                                .end_time
                                .as_ref()
                                .and_then(|end_time| end_time.duration_since(*start_time).ok())
                        })
                        .map(|duration| duration.as_millis())
                        .and_then(|duration| i64::try_from(duration).ok()),
                    &command_info.hostname,
                    &command_info.exit_code,
                ],
            )?;
        }

        Ok(())
    }

    pub fn all_rows(&self) -> Result<Vec<CommandInfo>> {
        let conn = self.conn()?;
        let mut stmt = conn.prepare(&format!("SELECT {ALL_COLUMNS} FROM history ORDER BY start_time ASC"))?;

        let rows = stmt.query([])?;

        let rows_mapped = rows.mapped(map_row).collect::<rusqlite::Result<Vec<CommandInfo>>>()?;

        Ok(rows_mapped)
    }

    /// Runs a read-only query against the `history` table and returns the rows as JSON objects.
    ///
    /// The query text comes from the autocomplete webview, which is not trusted with this
    /// database: `history`, `state` and `auth_kv` all live in the same sqlite file, so a bare
    /// `conn.prepare(query)` let any page loaded in that webview read the state store and the auth
    /// table, or write to any of them.
    ///
    /// The restriction is enforced by sqlite's own authorizer rather than by inspecting the SQL:
    /// it runs during statement *preparation* and sees the resolved table and column names, so it
    /// cannot be talked around with comments, quoting, CTEs, subqueries, or a second statement.
    /// Anything but reading `history` — a write, another table, a `PRAGMA`, an `ATTACH` of a
    /// second database file — is refused.
    pub fn query<P: rusqlite::Params>(
        &self,
        query: &str,
        params: P,
    ) -> Result<Vec<serde_json::Map<String, serde_json::Value>>> {
        let mut conn = self.conn()?;

        conn.authorizer(Some(history_reads_only))?;
        let result = Self::query_inner(&conn, query, params);
        // The connection is shared with the rest of the process, so the authorizer must come off
        // again whatever happened above.
        conn.authorizer::<Authorizer>(None)?;
        result
    }

    fn query_inner<P: rusqlite::Params>(
        conn: &Connection,
        query: &str,
        params: P,
    ) -> Result<Vec<serde_json::Map<String, serde_json::Value>>> {
        let mut stmt = conn.prepare(query)?;
        if !stmt.readonly() {
            return Err(Error::HistoryQueryNotAllowed);
        }
        let rows = stmt.query_map(params, |row| {
            let row_count = row.as_ref().column_count();
            let mut map = serde_json::Map::with_capacity(row_count);
            for i in 0..row_count {
                let name = row.as_ref().column_name(i)?;
                let value = match row.get_ref(i)? {
                    ValueRef::Null => Value::Null,
                    ValueRef::Integer(i) => Value::from(i),
                    ValueRef::Real(f) => Value::from(f),
                    ValueRef::Text(s) => Value::from(String::from_utf8_lossy(s)),
                    ValueRef::Blob(b) => Value::from(b),
                };
                map.insert(name.into(), value);
            }
            Ok(map)
        })?;

        let rows = rows.collect::<rusqlite::Result<Vec<serde_json::Map<String, serde_json::Value>>>>()?;

        Ok(rows)
    }
}

fn map_row(row: &rusqlite::Row<'_>) -> rusqlite::Result<CommandInfo> {
    let start_time = row
        .get::<_, Option<i64>>(6)?
        .and_then(|t| std::time::UNIX_EPOCH.checked_add(std::time::Duration::from_secs(u64::try_from(t).ok()?)));

    let duration = row
        .get::<_, Option<i64>>(7)?
        .and_then(|d| Some(std::time::Duration::from_millis(u64::try_from(d).ok()?)));

    let end_time = start_time
        .as_ref()
        .and_then(|start_time| duration.and_then(|duration| start_time.checked_add(duration)));

    Ok(CommandInfo {
        command: row.get(1)?,
        shell: row.get(2)?,
        pid: row.get(3)?,
        session_id: row.get(4)?,
        cwd: row.get(5)?,
        start_time,
        end_time,
        hostname: row.get(8)?,
        exit_code: row.get(9)?,
    })
}

#[cfg(test)]
mod tests {
    use fig_util::CLI_BINARY_NAME;

    use super::*;

    #[test]
    fn history_new_test() {
        let _ = History::new();
    }

    #[test]
    fn migrate_insert_query() {
        let history = History::mock();
        history.db().unwrap().migrate().unwrap();

        history
            .insert_command_history(&CommandInfo {
                command: Some(CLI_BINARY_NAME.into()),
                shell: Some("bash".into()),
                pid: Some(123),
                session_id: Some("session-id".into()),
                cwd: Some("/home/grant/".into()),
                start_time: Some(std::time::UNIX_EPOCH + std::time::Duration::from_secs(123)),
                end_time: Some(std::time::UNIX_EPOCH + std::time::Duration::from_secs(124)),
                hostname: Some("laptop".into()),
                exit_code: Some(0),
            })
            .unwrap();

        history
            .insert_command_history(&CommandInfo {
                command: Some("cargo test".into()),
                shell: Some("zsh".into()),
                pid: Some(124),
                session_id: Some("session-id".into()),
                cwd: Some("/home/grant/".into()),
                start_time: Some(std::time::UNIX_EPOCH + std::time::Duration::from_secs(124)),
                end_time: Some(std::time::UNIX_EPOCH + std::time::Duration::from_secs(125)),
                hostname: Some("laptop".into()),
                exit_code: Some(0),
            })
            .unwrap();

        history
            .insert_command_history(&CommandInfo {
                command: Some("cargo run".into()),
                shell: Some("zsh".into()),
                pid: Some(124),
                session_id: Some("session-id".into()),
                cwd: Some("/home/grant/".into()),
                start_time: Some(std::time::UNIX_EPOCH + std::time::Duration::from_secs(126)),
                end_time: None,
                hostname: Some("laptop".into()),
                exit_code: None,
            })
            .unwrap();

        let rows = history.all_rows().unwrap();
        assert_eq!(rows.len(), 3);

        assert_eq!(rows[0].command, Some(CLI_BINARY_NAME.into()));
        assert_eq!(rows[0].shell, Some("bash".into()));
        assert_eq!(rows[0].pid, Some(123));
        assert_eq!(rows[0].session_id, Some("session-id".into()));
        assert_eq!(rows[0].cwd, Some("/home/grant/".into()));
        assert_eq!(
            rows[0].start_time,
            Some(std::time::UNIX_EPOCH + std::time::Duration::from_secs(123))
        );
        assert_eq!(
            rows[0].end_time,
            Some(std::time::UNIX_EPOCH + std::time::Duration::from_secs(124))
        );
        assert_eq!(rows[0].hostname, Some("laptop".into()));
        assert_eq!(rows[0].exit_code, Some(0));

        assert_eq!(rows[1].command, Some("cargo test".into()));
        assert_eq!(rows[1].shell, Some("zsh".into()));
        assert_eq!(rows[1].pid, Some(124));
        assert_eq!(rows[1].session_id, Some("session-id".into()));
        assert_eq!(rows[1].cwd, Some("/home/grant/".into()));
        assert_eq!(
            rows[1].start_time,
            Some(std::time::UNIX_EPOCH + std::time::Duration::from_secs(124))
        );
        assert_eq!(
            rows[1].end_time,
            Some(std::time::UNIX_EPOCH + std::time::Duration::from_secs(125))
        );
        assert_eq!(rows[1].hostname, Some("laptop".into()));
        assert_eq!(rows[1].exit_code, Some(0));

        assert_eq!(rows[2].command, Some("cargo run".into()));
        assert_eq!(rows[2].shell, Some("zsh".into()));
        assert_eq!(rows[2].pid, Some(124));
        assert_eq!(rows[2].session_id, Some("session-id".into()));
        assert_eq!(rows[2].cwd, Some("/home/grant/".into()));
        assert_eq!(
            rows[2].start_time,
            Some(std::time::UNIX_EPOCH + std::time::Duration::from_secs(126))
        );
        assert_eq!(rows[2].end_time, None);
        assert_eq!(rows[2].hostname, Some("laptop".into()));
        assert_eq!(rows[2].exit_code, None);

        // The dead `rows()` builder used to be exercised here; the same two queries go through
        // `query`, which is the only surviving read path.
        let row = history
            .query("SELECT command FROM history ORDER BY id DESC LIMIT 1", ())
            .unwrap();
        assert_eq!(row.len(), 1);
        assert_eq!(row[0]["command"], serde_json::json!("cargo run"));

        let row = history
            .query("SELECT id FROM history WHERE exit_code IS NOT NULL", ())
            .unwrap();
        assert_eq!(row.len(), 2);

        // while we're here, test the `query` method
        let row = history.query("SELECT * FROM history ORDER BY id ASC", ()).unwrap();

        assert_eq!(row.len(), 3);

        assert_eq!(
            &row[0],
            serde_json::json!({
                "id": 1,
                "command": CLI_BINARY_NAME,
                "shell": "bash",
                "pid": 123,
                "session_id": "session-id",
                "cwd": "/home/grant/",
                "start_time": 123,
                "end_time": 124,
                "hostname": "laptop",
                "exit_code": 0,
                "duration": 1000,
            })
            .as_object()
            .unwrap()
        );

        assert_eq!(
            &row[1],
            serde_json::json!({
                "id": 2,
                "command": "cargo test",
                "shell": "zsh",
                "pid": 124,
                "session_id": "session-id",
                "cwd": "/home/grant/",
                "start_time": 124,
                "end_time": 125,
                "hostname": "laptop",
                "exit_code": 0,
                "duration": 1000,
            })
            .as_object()
            .unwrap()
        );

        assert_eq!(
            &row[2],
            serde_json::json!({
                "id": 3,
                "command": "cargo run",
                "shell": "zsh",
                "pid": 124,
                "session_id": "session-id",
                "cwd": "/home/grant/",
                "start_time": 126,
                "end_time": null,
                "hostname": "laptop",
                "exit_code": null,
                "duration": null,
            })
            .as_object()
            .unwrap()
        );
    }

    /// The query text comes from the autocomplete webview. `history`, `state` and `auth_kv` share
    /// one sqlite file, so an unrestricted `conn.prepare(query)` handed any page loaded in that
    /// webview the state store and the auth table.
    #[test]
    fn query_can_only_read_the_history_table() {
        let history = History::mock();
        history
            .insert_command_history(&CommandInfo {
                command: Some("echo hi".into()),
                ..Default::default()
            })
            .unwrap();
        // Something to try to steal.
        history
            .db()
            .unwrap()
            .conn()
            .execute(
                "INSERT OR REPLACE INTO state (key, value) VALUES ('secret', '\"hunter2\"')",
                [],
            )
            .unwrap();

        // The query the app actually issues still works.
        let rows = history
            .query(
                "SELECT command, shell, session_id, cwd, start_time, exit_code FROM history",
                (),
            )
            .unwrap();
        assert_eq!(rows.len(), 1);

        for forbidden in [
            "SELECT * FROM state",
            "SELECT * FROM auth_kv",
            "SELECT * FROM sqlite_master",
            "SELECT (SELECT value FROM state WHERE key = 'secret') AS x FROM history",
            "WITH s AS (SELECT value FROM state) SELECT * FROM s",
            "SELECT command FROM history UNION SELECT value FROM state",
            "DELETE FROM history",
            "UPDATE state SET value = '1'",
            "INSERT INTO history (command) VALUES ('x')",
            "DROP TABLE history",
            "ATTACH DATABASE '/tmp/evil.sqlite3' AS evil",
            "PRAGMA journal_mode = DELETE",
            "CREATE TABLE evil (x)",
        ] {
            assert!(history.query(forbidden, ()).is_err(), "{forbidden:?} was allowed");
        }

        // Everything still works afterwards: the authorizer must not be left installed on the
        // shared connection.
        assert_eq!(history.all_rows().unwrap().len(), 1);
        assert_eq!(history.query("SELECT command FROM history", ()).unwrap().len(), 1);
    }
}
