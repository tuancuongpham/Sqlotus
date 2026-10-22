//! Run one script against several databases of the same server
//! (multi-tenant setups), tagging every row with its database.

use sqlotus_core::byte_to_utf16;
use sqlotus_core::safety::ScriptRun;
use sqlotus_db::{
    BatchOutput, CancelHandle, Connection, Limits, Message, MessageKind, ResultColumn, ResultSet, execute_script,
};
use tauri::State;

use crate::commands::{BatchDto, CmdResult, ExecuteResult, err};
use crate::state::{AppState, Running};

fn tag(db: &str, mut rs: ResultSet) -> ResultSet {
    rs.columns.insert(
        0,
        ResultColumn {
            name: "Database".into(),
            type_name: "sysname".into(),
        },
    );
    for row in &mut rs.rows {
        row.insert(0, Some(db.to_string()));
    }
    rs
}

/// Combine per-database outputs. When every database returned result sets
/// with the same columns, the k-th sets are merged into one grid; otherwise
/// each database keeps its own batch.
fn merge(per_db: Vec<(String, Vec<BatchOutput>)>) -> Vec<BatchOutput> {
    let shapes: Vec<Vec<Vec<String>>> = per_db
        .iter()
        .map(|(_, outs)| {
            outs.iter()
                .flat_map(|o| &o.result_sets)
                .map(|rs| rs.columns.iter().map(|c| c.name.to_lowercase()).collect())
                .collect()
        })
        .collect();
    let mergeable = !per_db.is_empty()
        && per_db.iter().all(|(_, outs)| outs.iter().all(|o| !o.failed))
        && shapes.windows(2).all(|w| w[0] == w[1])
        && !shapes[0].is_empty();
    if !mergeable {
        return per_db
            .into_iter()
            .flat_map(|(db, outs)| {
                outs.into_iter().map(move |mut o| {
                    o.result_sets = o.result_sets.into_iter().map(|rs| tag(&db, rs)).collect();
                    for m in &mut o.messages {
                        m.text = format!("[{db}] {}", m.text);
                    }
                    o
                })
            })
            .collect();
    }
    let mut merged = BatchOutput::default();
    for (db, outs) in per_db {
        let sets: Vec<ResultSet> = outs.iter().flat_map(|o| o.result_sets.clone()).collect();
        for (k, rs) in sets.into_iter().enumerate() {
            let rs = tag(&db, rs);
            match merged.result_sets.get_mut(k) {
                Some(target) => {
                    target.truncated |= rs.truncated;
                    target.clipped += rs.clipped;
                    target.rows.extend(rs.rows);
                }
                None => merged.result_sets.push(rs),
            }
        }
        for o in outs {
            merged.elapsed_ms += o.elapsed_ms;
            merged.messages.extend(o.messages.into_iter().map(|m| Message {
                kind: m.kind,
                text: format!("[{db}] {}", m.text),
            }));
        }
    }
    vec![merged]
}

#[tauri::command]
pub async fn execute_multi(
    state: State<'_, AppState>,
    tab_id: String,
    sql: String,
    databases: Vec<String>,
) -> CmdResult<ExecuteResult> {
    let session = state.session(&tab_id).ok_or("tab is not connected")?;
    let as_run = sqlotus_core::sqlcmd::as_run(&sql, session.config.dialect);
    let run = ScriptRun::new(&as_run, session.config.dialect);
    let changes = !run.changes().is_empty();
    if session.config.read_only && changes {
        return Err("This connection is read-only, so the script was not run on any database.".into());
    }
    // procedures change rows too, which manual commit keeps uncommitted
    if session.config.manual_commit && (changes || !run.calls().is_empty()) {
        return Err("Manual commit is on for this connection, but a multi-database run commits each database on its own connection. Run the changes database by database instead.".into());
    }
    let schema_changed = run.changes_schema();
    let limits = Limits {
        max_rows: state.settings().editor.max_rows.max(1),
        ..Limits::default()
    };
    // run in a task Cancel can stop: it cancels the query running, and the
    // databases after it are not started (cancel_query takes `running`)
    let (go, ready) = tokio::sync::oneshot::channel::<()>();
    let task_session = session.clone();
    let script = sql.clone();
    let handle = tauri::async_runtime::spawn(async move {
        let mut per_db = Vec::new();
        if ready.await.is_err() {
            return (per_db, true);
        }
        let cancelled = || task_session.running.lock().map_or(true, |r| r.is_none());
        for db in databases {
            if cancelled() {
                return (per_db, true);
            }
            let mut config = task_session.config.clone();
            config.database = Some(db.clone());
            let outs = match Connection::open(&config, task_session.password.as_deref()).await {
                Ok(mut conn) => {
                    if let Ok(mut running) = task_session.running.lock()
                        && let Some(r) = running.as_mut()
                    {
                        r.cancel = conn.cancel_handle();
                    }
                    let outs: Vec<BatchOutput> = execute_script(&mut conn, &script, &limits)
                        .await
                        .into_iter()
                        .map(|b| b.output)
                        .collect();
                    conn.close().await;
                    outs
                }
                Err(e) => vec![BatchOutput {
                    messages: vec![Message {
                        kind: MessageKind::Error,
                        text: e.to_string(),
                    }],
                    failed: true,
                    ..BatchOutput::default()
                }],
            };
            per_db.push((db, outs));
        }
        let cancelled = cancelled();
        (per_db, cancelled)
    });
    *session.running.lock().map_err(err)? = Some(Running {
        abort: handle.inner().abort_handle(),
        cancel: CancelHandle::None,
    });
    let _ = go.send(());
    let result = handle.await;
    session.running.lock().map_err(err)?.take();
    let database = session.database.lock().map(|d| d.clone()).unwrap_or_default();
    // aborted (SQL Server cancels by dropping the connection): nothing to show
    let Ok((per_db, cancelled)) = result else {
        return Ok(ExecuteResult {
            database,
            cancelled: true,
            ..ExecuteResult::default()
        });
    };
    let end = byte_to_utf16(&sql, sql.len());
    Ok(ExecuteResult {
        batches: merge(per_db)
            .into_iter()
            .map(|output| BatchDto { start: 0, end, output })
            .collect(),
        database,
        cancelled,
        open_transactions: 0,
        schema_changed,
        ..ExecuteResult::default()
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn out(cols: &[&str], rows: usize) -> BatchOutput {
        BatchOutput {
            result_sets: vec![ResultSet {
                columns: cols
                    .iter()
                    .map(|c| ResultColumn {
                        name: (*c).into(),
                        type_name: String::new(),
                    })
                    .collect(),
                rows: (0..rows).map(|i| vec![Some(i.to_string()); cols.len()]).collect(),
                truncated: false,
                clipped: 0,
            }],
            ..BatchOutput::default()
        }
    }

    #[test]
    fn same_shape_is_merged() {
        let merged = merge(vec![
            ("a".into(), vec![out(&["x"], 2)]),
            ("b".into(), vec![out(&["x"], 1)]),
        ]);
        assert_eq!(merged.len(), 1);
        let rs = &merged[0].result_sets[0];
        assert_eq!(rs.columns[0].name, "Database");
        assert_eq!(rs.rows.len(), 3);
        assert_eq!(rs.rows[2][0].as_deref(), Some("b"));
    }

    #[test]
    fn different_shapes_stay_apart() {
        let merged = merge(vec![
            ("a".into(), vec![out(&["x"], 1)]),
            ("b".into(), vec![out(&["y"], 1)]),
        ]);
        assert_eq!(merged.len(), 2);
    }
}
