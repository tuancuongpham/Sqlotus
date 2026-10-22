//! The SQL Server Backup dialog: what the server says (default folders,
//! edition, recovery model, what a backup file holds) and the BACKUP /
//! RESTORE scripts. Only reads run here; the scripts open in a tab to be
//! reviewed and run by the user.

use serde::Serialize;
use sqlotus_core::Dialect;
use sqlotus_core::backup::{self, BackupFile, BackupOptions, RestoreMove, RestoreOptions};
use sqlotus_db::{Connection, Limits, ResultSet};
use tauri::State;

use crate::commands::{CmdResult, no_panic, open_database};
use crate::state::AppState;

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct BackupInfo {
    /// The server's default folders ("" when it does not say).
    backup_dir: String,
    data_dir: String,
    log_dir: String,
    /// Express edition: no backup compression.
    express: bool,
    /// The database's recovery model: SIMPLE has no log backups.
    recovery_model: String,
}

fn text(s: &str) -> String {
    format!("N{}", Dialect::MsSql.string_literal(s))
}

/// A short-lived connection to the server (the tab's stays free).
async fn server(state: &AppState, connection_id: &str, database: &str) -> CmdResult<Connection> {
    let conn = open_database(state, connection_id, database).await?;
    if conn.dialect() != Dialect::MsSql {
        conn.close().await;
        return Err("Backup and restore scripts are for SQL Server.".into());
    }
    Ok(conn)
}

/// Run `sql` and return its first result set, or the server's error.
async fn result(conn: &mut Connection, sql: &str) -> CmdResult<ResultSet> {
    let out = conn
        .execute(
            sql,
            &Limits {
                max_rows: 10_000,
                max_cell_chars: usize::MAX,
            },
        )
        .await;
    if out.failed {
        return Err(out.errors());
    }
    out.result_sets
        .into_iter()
        .next()
        .ok_or_else(|| "The server returned nothing.".to_string())
}

/// The value of `column` in `row`, by name.
fn cell(rs: &ResultSet, row: &[Option<String>], column: &str) -> String {
    rs.columns
        .iter()
        .position(|c| c.name.eq_ignore_ascii_case(column))
        .and_then(|i| row.get(i).cloned().flatten())
        .unwrap_or_default()
}

#[tauri::command]
pub async fn backup_info(state: State<'_, AppState>, connection_id: String, database: String) -> CmdResult<BackupInfo> {
    let mut conn = server(&state, &connection_id, &database).await?;
    let sql = format!(
        "SELECT CAST(SERVERPROPERTY('InstanceDefaultBackupPath') AS nvarchar(4000)) AS backup_dir, \
         CAST(SERVERPROPERTY('InstanceDefaultDataPath') AS nvarchar(4000)) AS data_dir, \
         CAST(SERVERPROPERTY('InstanceDefaultLogPath') AS nvarchar(4000)) AS log_dir, \
         CAST(SERVERPROPERTY('EngineEdition') AS int) AS edition, \
         (SELECT recovery_model_desc FROM sys.databases WHERE name = {}) AS recovery_model",
        text(&database)
    );
    let rs = result(&mut conn, &sql).await;
    conn.close().await;
    let rs = rs?;
    let row = rs.rows.first().cloned().unwrap_or_default();
    Ok(BackupInfo {
        backup_dir: cell(&rs, &row, "backup_dir"),
        data_dir: cell(&rs, &row, "data_dir"),
        log_dir: cell(&rs, &row, "log_dir"),
        express: cell(&rs, &row, "edition") == "4",
        recovery_model: cell(&rs, &row, "recovery_model"),
    })
}

/// A backup in a backup file (RESTORE HEADERONLY).
#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct BackupSet {
    position: u32,
    database_name: String,
    kind: String,
    finished: String,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct BackupContents {
    sets: Vec<BackupSet>,
    /// The database files of backup `file_number`.
    files: Vec<BackupFile>,
}

/// What a backup file on the server holds: its backups, and the database
/// files of one of them. Reads the file only.
#[tauri::command]
pub async fn backup_contents(
    state: State<'_, AppState>,
    connection_id: String,
    database: String,
    path: String,
    file_number: u32,
) -> CmdResult<BackupContents> {
    let mut conn = server(&state, &connection_id, &database).await?;
    let disk = text(path.trim());
    let headers = result(&mut conn, &format!("RESTORE HEADERONLY FROM DISK = {disk}")).await;
    let files = match &headers {
        Ok(_) => {
            result(
                &mut conn,
                &format!(
                    "RESTORE FILELISTONLY FROM DISK = {disk} WITH FILE = {}",
                    file_number.max(1)
                ),
            )
            .await
        }
        Err(e) => Err(e.clone()),
    };
    conn.close().await;
    let (headers, files) = (headers?, files?);
    let sets = headers
        .rows
        .iter()
        .map(|r| BackupSet {
            position: cell(&headers, r, "Position").parse().unwrap_or(0),
            database_name: cell(&headers, r, "DatabaseName"),
            kind: match cell(&headers, r, "BackupType").as_str() {
                "1" => "full",
                "2" => "log",
                "5" => "differential",
                _ => "other",
            }
            .into(),
            finished: cell(&headers, r, "BackupFinishDate"),
        })
        .collect();
    let files = files
        .rows
        .iter()
        .map(|r| BackupFile {
            logical_name: cell(&files, r, "LogicalName"),
            physical_name: cell(&files, r, "PhysicalName"),
            kind: cell(&files, r, "Type"),
        })
        .collect();
    Ok(BackupContents { sets, files })
}

/// Where the files go when the backup is restored as `database`.
#[tauri::command]
pub fn restore_moves(files: Vec<BackupFile>, database: String, data_dir: String, log_dir: String) -> Vec<RestoreMove> {
    backup::default_moves(&files, &database, &data_dir, &log_dir)
}

#[tauri::command]
pub fn backup_script(options: BackupOptions) -> CmdResult<String> {
    no_panic("backup script", || backup::backup_script(&options))?
}

#[tauri::command]
pub fn restore_script(options: RestoreOptions) -> CmdResult<String> {
    no_panic("restore script", || backup::restore_script(&options))?
}
