//! Run one script against several databases of the same server
//! (multi-tenant setups), tagging every row with its database.

use sqlotus_core::byte_to_utf16;
use sqlotus_db::{BatchOutput, Connection, Limits, Message, MessageKind, ResultColumn, ResultSet, execute_script};
use tauri::State;

use crate::commands::{BatchDto, CmdResult, ExecuteResult};
use crate::state::AppState;

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
    if session.config.read_only
        && !sqlotus_core::analysis::modifying_statements(&sql, session.config.dialect).is_empty()
    {
        return Err("This connection is read-only, so the script was not run on any database.".into());
    }
    if session.config.manual_commit
        && !sqlotus_core::analysis::modifying_statements(&sql, session.config.dialect).is_empty()
    {
        return Err("Manual commit is on for this connection, but a multi-database run commits each database on its own connection. Run the changes database by database instead.".into());
    }
    let limits = Limits {
        max_rows: state.settings().editor.max_rows.max(1),
        ..Limits::default()
    };
    let mut per_db = Vec::new();
    for db in databases {
        let mut config = session.config.clone();
        config.database = Some(db.clone());
        let outs = match Connection::open(&config, session.password.as_deref()).await {
            Ok(mut conn) => {
                let outs: Vec<BatchOutput> = execute_script(&mut conn, &sql, &limits)
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
    let end = byte_to_utf16(&sql, sql.len());
    let database = session.database.lock().map(|d| d.clone()).unwrap_or_default();
    Ok(ExecuteResult {
        batches: merge(per_db)
            .into_iter()
            .map(|output| BatchDto { start: 0, end, output })
            .collect(),
        database,
        cancelled: false,
        open_transactions: 0,
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
