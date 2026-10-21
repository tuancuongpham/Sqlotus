//! Activity monitor: sessions on the server, what they run and wait for,
//! and who blocks whom. Uses its own connection per server so a running
//! query in a tab does not hold it up.

use serde::Serialize;
use sqlotus_core::Dialect;
use tauri::State;

use crate::commands::{CmdResult, err, open_database};
use crate::state::AppState;

#[derive(Serialize, Clone)]
#[serde(rename_all = "camelCase")]
pub struct SessionInfo {
    id: i64,
    /// Session this one waits for (0 = none).
    blocked_by: i64,
    /// How many sessions wait for this one, directly or not.
    blocking: usize,
    login: String,
    host: String,
    program: String,
    database: String,
    status: String,
    command: String,
    wait: String,
    elapsed_ms: i64,
    cpu_ms: Option<i64>,
    reads: Option<i64>,
    open_transaction: bool,
    sql: String,
    running: bool,
    /// The monitor's own session.
    is_self: bool,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Activity {
    sessions: Vec<SessionInfo>,
    note: Option<String>,
}

// one column order for all dialects:
// id, blocked_by, login, host, program, database, status, command, wait, elapsed_ms, cpu_ms, reads, open_tran, sql, running, is_self
const MSSQL: &str = "
SELECT s.session_id, ISNULL(r.blocking_session_id, 0), s.login_name, s.host_name, s.program_name,
  DB_NAME(COALESCE(r.database_id, s.database_id)), COALESCE(r.status, s.status), r.command, r.wait_type,
  COALESCE(r.total_elapsed_time, DATEDIFF(ms, s.last_request_start_time, GETDATE())), COALESCE(r.cpu_time, s.cpu_time),
  COALESCE(r.logical_reads, s.logical_reads), s.open_transaction_count,
  COALESCE(SUBSTRING(t.text, r.statement_start_offset / 2 + 1,
    (CASE WHEN r.statement_end_offset = -1 THEN DATALENGTH(t.text) ELSE r.statement_end_offset END - r.statement_start_offset) / 2 + 1), t.text),
  CASE WHEN r.session_id IS NULL THEN 0 ELSE 1 END, CASE WHEN s.session_id = @@SPID THEN 1 ELSE 0 END
FROM sys.dm_exec_sessions s
LEFT JOIN sys.dm_exec_requests r ON r.session_id = s.session_id
LEFT JOIN sys.dm_exec_connections c ON c.session_id = s.session_id
OUTER APPLY sys.dm_exec_sql_text(COALESCE(r.sql_handle, c.most_recent_sql_handle)) t
WHERE s.is_user_process = 1";

const POSTGRES: &str = "
SELECT pid, COALESCE((pg_blocking_pids(pid))[1], 0), usename, COALESCE(client_hostname, client_addr::text, 'local'), application_name,
  datname, state, NULL, COALESCE(wait_event_type || ': ' || wait_event, ''),
  (EXTRACT(EPOCH FROM (now() - COALESCE(query_start, backend_start))) * 1000)::bigint, NULL, NULL,
  CASE WHEN xact_start IS NULL THEN 0 ELSE 1 END, query, CASE WHEN state = 'active' THEN 1 ELSE 0 END,
  CASE WHEN pid = pg_backend_pid() THEN 1 ELSE 0 END
FROM pg_stat_activity WHERE backend_type = 'client backend'";

const MYSQL: &str = "
SELECT p.ID, COALESCE(w.blocking_pid, 0), p.USER, p.HOST, '', p.DB, p.COMMAND, p.STATE, '', p.TIME * 1000, NULL, NULL,
  CASE WHEN t.trx_id IS NULL THEN 0 ELSE 1 END, p.INFO, CASE WHEN p.COMMAND <> 'Sleep' THEN 1 ELSE 0 END,
  CASE WHEN p.ID = CONNECTION_ID() THEN 1 ELSE 0 END
FROM information_schema.PROCESSLIST p
LEFT JOIN information_schema.INNODB_TRX t ON t.trx_mysql_thread_id = p.ID
LEFT JOIN (SELECT waiting_pid, MIN(blocking_pid) AS blocking_pid FROM sys.innodb_lock_waits GROUP BY waiting_pid) w ON w.waiting_pid = p.ID";

fn monitor_key(connection_id: &str, database: &str) -> String {
    format!("{connection_id}\u{1}{database}")
}

/// Sessions of the server, with blocking counts.
#[tauri::command]
pub async fn server_activity(
    state: State<'_, AppState>,
    connection_id: String,
    database: String,
    dialect: Dialect,
) -> CmdResult<Activity> {
    let key = monitor_key(&connection_id, &database);
    let mut monitors = state.monitors.lock().await;
    let sql = match dialect {
        Dialect::MsSql => MSSQL.to_string(),
        Dialect::Postgres => POSTGRES.to_string(),
        Dialect::MySql => MYSQL.to_string(),
    };
    let mut note = None;
    let mut rows = None;
    for attempt in 0..2 {
        if !monitors.contains_key(&key) {
            let c = open_database(&state, &connection_id, &database).await?;
            monitors.insert(key.clone(), c);
        }
        let conn = monitors.get_mut(&key).expect("inserted");
        match conn.query_strings(&sql).await {
            Ok(r) => {
                rows = Some(r);
                break;
            }
            // MySQL without the sys schema: no lock waits, the rest still works
            Err(_) if dialect == Dialect::MySql && attempt == 0 => {
                let plain = MYSQL.replace(
                    "LEFT JOIN (SELECT waiting_pid, MIN(blocking_pid) AS blocking_pid FROM sys.innodb_lock_waits GROUP BY waiting_pid) w ON w.waiting_pid = p.ID",
                    "LEFT JOIN (SELECT NULL AS waiting_pid, NULL AS blocking_pid) w ON 1 = 0",
                );
                if let Ok(r) = conn.query_strings(&plain).await {
                    note = Some(
                        "Lock waits are not available (the sys schema is missing), so blocking is not shown.".into(),
                    );
                    rows = Some(r);
                    break;
                }
            }
            Err(e) if attempt == 0 => {
                // the monitor connection may have dropped: reconnect once
                monitors.remove(&key);
                if e.to_string().contains("permission") || e.to_string().contains("VIEW SERVER STATE") {
                    return Err(format!(
                        "{e}\nSeeing other sessions needs the VIEW SERVER STATE permission."
                    ));
                }
            }
            Err(e) => return Err(err(e)),
        }
    }
    let rows = rows.ok_or("The activity query did not return.")?;
    let s = |r: &[Option<String>], i: usize| r.get(i).cloned().flatten().unwrap_or_default();
    let n = |r: &[Option<String>], i: usize| {
        r.get(i)
            .cloned()
            .flatten()
            .and_then(|v| v.split('.').next().and_then(|x| x.parse::<i64>().ok()))
    };
    let mut sessions: Vec<SessionInfo> = rows
        .iter()
        .map(|r| SessionInfo {
            id: n(r, 0).unwrap_or(0),
            blocked_by: n(r, 1).unwrap_or(0),
            blocking: 0,
            login: s(r, 2),
            host: s(r, 3),
            program: s(r, 4),
            database: s(r, 5),
            status: s(r, 6),
            command: s(r, 7),
            wait: s(r, 8),
            elapsed_ms: n(r, 9).unwrap_or(0).max(0),
            cpu_ms: n(r, 10),
            reads: n(r, 11),
            open_transaction: n(r, 12).unwrap_or(0) > 0,
            sql: s(r, 13).trim().to_string(),
            running: n(r, 14).unwrap_or(0) == 1,
            is_self: n(r, 15).unwrap_or(0) == 1,
        })
        .collect();
    // count everyone waiting behind each session (whole chain)
    let edges: Vec<(i64, i64)> = sessions
        .iter()
        .filter(|x| x.blocked_by != 0 && x.blocked_by != x.id)
        .map(|x| (x.id, x.blocked_by))
        .collect();
    for sess in &mut sessions {
        let mut seen = vec![sess.id];
        let mut frontier = vec![sess.id];
        while let Some(b) = frontier.pop() {
            for (waiter, blocker) in &edges {
                if *blocker == b && !seen.contains(waiter) {
                    seen.push(*waiter);
                    frontier.push(*waiter);
                }
            }
        }
        sess.blocking = seen.len() - 1;
    }
    sessions.sort_by(|a, b| {
        b.blocking
            .cmp(&a.blocking)
            .then(b.running.cmp(&a.running))
            .then(b.elapsed_ms.cmp(&a.elapsed_ms))
    });
    Ok(Activity { sessions, note })
}

/// Cancel the running query of a session, or end the session.
#[tauri::command]
pub async fn kill_session(
    state: State<'_, AppState>,
    connection_id: String,
    database: String,
    dialect: Dialect,
    id: i64,
    whole_session: bool,
) -> CmdResult<String> {
    if id <= 0 {
        return Err("Invalid session id.".into());
    }
    let sql = match (dialect, whole_session) {
        (Dialect::MsSql, _) => format!("KILL {id}"),
        (Dialect::Postgres, false) => format!("SELECT pg_cancel_backend({id})"),
        (Dialect::Postgres, true) => format!("SELECT pg_terminate_backend({id})"),
        (Dialect::MySql, false) => format!("KILL QUERY {id}"),
        (Dialect::MySql, true) => format!("KILL {id}"),
    };
    let key = monitor_key(&connection_id, &database);
    let mut monitors = state.monitors.lock().await;
    if !monitors.contains_key(&key) {
        let c = open_database(&state, &connection_id, &database).await?;
        monitors.insert(key.clone(), c);
    }
    let conn = monitors.get_mut(&key).expect("inserted");
    conn.query_strings(&sql)
        .await
        .map_err(|e| format!("{sql} failed: {e}"))?;
    Ok(if whole_session || dialect == Dialect::MsSql {
        format!("Session {id} was ended; its open transaction (if any) is rolled back.")
    } else {
        format!("The running query of session {id} was cancelled.")
    })
}

/// Close the monitor connection (when the monitor window closes).
#[tauri::command]
pub async fn close_activity(state: State<'_, AppState>, connection_id: String, database: String) -> CmdResult<()> {
    if let Some(c) = state
        .monitors
        .lock()
        .await
        .remove(&monitor_key(&connection_id, &database))
    {
        c.close().await;
    }
    Ok(())
}
