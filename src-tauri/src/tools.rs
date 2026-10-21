//! Database tools beyond the editor: the ER diagram model and test data.

use std::sync::Arc;

use serde::Serialize;
use sqlotus_core::Dialect;
use sqlotus_core::catalog::{DbObject, ForeignKey, ObjectKind};
use sqlotus_core::editing::{EditPlan, EditSource, RowEdit};
use sqlotus_core::testdata::{self, ColumnPlan, DataContext, Generator, TestDataOptions};
use sqlotus_db::CatalogSnapshot;
use tauri::State;

use crate::commands::{CmdResult, fetch_catalog, no_panic, open_database};
use crate::state::AppState;

async fn snapshot(state: &AppState, connection_id: &str, database: &str) -> CmdResult<Arc<CatalogSnapshot>> {
    match state.cached_catalog(connection_id, database) {
        Some(s) => Ok(s),
        None => fetch_catalog(state, connection_id, database).await,
    }
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct DiagramModel {
    tables: Vec<DbObject>,
    foreign_keys: Vec<ForeignKey>,
}

/// Tables and foreign keys of a database, for the ER diagram.
#[tauri::command]
pub async fn diagram_model(
    state: State<'_, AppState>,
    connection_id: String,
    database: String,
) -> CmdResult<DiagramModel> {
    let snap = snapshot(&state, &connection_id, &database).await?;
    let tables: Vec<DbObject> = snap
        .catalog
        .objects
        .iter()
        .filter(|o| o.kind == ObjectKind::Table)
        .cloned()
        .collect();
    // only relations between tables that are in the model
    let known = |schema: &str, name: &str| {
        tables
            .iter()
            .any(|t| t.schema.eq_ignore_ascii_case(schema) && t.name.eq_ignore_ascii_case(name))
    };
    let foreign_keys = snap
        .catalog
        .foreign_keys
        .iter()
        .filter(|f| known(&f.from_schema, &f.from_table) && known(&f.to_schema, &f.to_table))
        .cloned()
        .collect();
    Ok(DiagramModel { tables, foreign_keys })
}

fn find_table<'a>(snap: &'a CatalogSnapshot, schema: &str, name: &str) -> CmdResult<&'a DbObject> {
    snap.catalog
        .objects
        .iter()
        .find(|o| {
            o.kind == ObjectKind::Table && o.schema.eq_ignore_ascii_case(schema) && o.name.eq_ignore_ascii_case(name)
        })
        .ok_or_else(|| format!("Table {schema}.{name} is not in the loaded objects; reload objects and try again."))
}

/// The generator Sqlotus would use for each column.
#[tauri::command]
pub async fn test_data_plan(
    state: State<'_, AppState>,
    connection_id: String,
    database: String,
    schema: String,
    name: String,
    dialect: Dialect,
) -> CmdResult<Vec<ColumnPlan>> {
    let snap = snapshot(&state, &connection_id, &database).await?;
    let table = find_table(&snap, &schema, &name)?;
    no_panic("test data", || {
        testdata::plan_columns(table, &snap.catalog.foreign_keys, dialect)
    })
}

fn qualified(dialect: Dialect, schema: &str, name: &str) -> String {
    if schema.is_empty() {
        dialect.quote_ident(name)
    } else {
        format!("{}.{}", dialect.quote_ident(schema), dialect.quote_ident(name))
    }
}

/// An INSERT script with generated rows. Foreign keys take values from the
/// referenced tables and sequence columns continue after their maximum, so
/// the database is read (never written).
#[tauri::command]
#[allow(clippy::too_many_arguments)]
pub async fn test_data_script(
    state: State<'_, AppState>,
    connection_id: String,
    database: String,
    schema: String,
    name: String,
    dialect: Dialect,
    options: TestDataOptions,
) -> CmdResult<String> {
    if options.rows == 0 || options.rows > 100_000 {
        return Err("Choose between 1 and 100,000 rows.".into());
    }
    let snap = snapshot(&state, &connection_id, &database).await?;
    let table = find_table(&snap, &schema, &name)?.clone();
    let fks = snap.catalog.foreign_keys.clone();
    let plans = testdata::plan_columns(&table, &fks, dialect);
    let generator = |column: &str| {
        options
            .overrides
            .get(column)
            .copied()
            .or_else(|| plans.iter().find(|p| p.column == column).map(|p| p.generator))
    };

    let mut ctx = DataContext::default();
    let needs_fk = testdata::foreign_keys_of(&table, &fks)
        .into_iter()
        .filter(|f| {
            f.from_columns
                .iter()
                .any(|c| generator(c) == Some(Generator::ForeignKey))
        })
        .cloned()
        .collect::<Vec<_>>();
    let sequences: Vec<String> = plans
        .iter()
        .filter(|p| generator(&p.column) == Some(Generator::Sequence))
        .map(|p| p.column.clone())
        .collect();
    if !needs_fk.is_empty() || !sequences.is_empty() {
        let mut conn = open_database(&state, &connection_id, &database).await?;
        for f in &needs_fk {
            let cols = f
                .to_columns
                .iter()
                .map(|c| dialect.quote_ident(c))
                .collect::<Vec<_>>()
                .join(", ");
            let not_null = f
                .to_columns
                .iter()
                .map(|c| format!("{} IS NOT NULL", dialect.quote_ident(c)))
                .collect::<Vec<_>>()
                .join(" AND ");
            let from = qualified(dialect, &f.to_schema, &f.to_table);
            let sql = match dialect {
                Dialect::MsSql => format!("SELECT DISTINCT TOP (1000) {cols} FROM {from} WHERE {not_null}"),
                _ => format!("SELECT DISTINCT {cols} FROM {from} WHERE {not_null} LIMIT 1000"),
            };
            let rows = conn
                .query_strings(&sql)
                .await
                .map_err(|e| format!("Could not read {from}: {e}"))?;
            ctx.parent_rows.insert(f.name.clone(), rows);
        }
        let target = qualified(dialect, &table.schema, &table.name);
        for col in &sequences {
            let sql = format!("SELECT MAX({}) FROM {target}", dialect.quote_ident(col));
            let max = conn
                .query_strings(&sql)
                .await
                .map_err(|e| format!("Could not read the largest {col}: {e}"))?
                .first()
                .and_then(|r| r.first().cloned().flatten())
                .and_then(|v| v.parse::<i64>().ok())
                .unwrap_or(0);
            ctx.max_values.insert(col.to_lowercase(), max);
        }
        conn.close().await;
    }
    no_panic("test data", || {
        testdata::insert_script(&table, &plans, &fks, &ctx, &options, dialect)
    })?
}

// ---------------------------------------------------------------------------
// dependencies
// ---------------------------------------------------------------------------

/// Views, procedures, functions, triggers and foreign keys that use a table,
/// a column or a module.
#[tauri::command]
pub async fn object_references(
    state: State<'_, AppState>,
    connection_id: String,
    database: String,
    schema: String,
    name: String,
    column: Option<String>,
    dialect: Dialect,
) -> CmdResult<Vec<sqlotus_core::deps::Reference>> {
    let snap = snapshot(&state, &connection_id, &database).await?;
    no_panic("dependencies", || {
        sqlotus_core::deps::find_references(
            &snap.definitions,
            &snap.catalog,
            dialect,
            &schema,
            &name,
            column.as_deref(),
        )
    })
}

/// Statements that change or may change data or schema (for the production
/// confirmation): modifying statements plus procedure calls.
#[tauri::command]
pub fn modifying_statements(text: String, dialect: Dialect) -> CmdResult<Vec<String>> {
    no_panic("analysis", || {
        let mut out = sqlotus_core::analysis::modifying_statements(&text, dialect);
        out.extend(sqlotus_core::analysis::routine_calls(&text, dialect));
        out
    })
}

// ---------------------------------------------------------------------------
// editing results
// ---------------------------------------------------------------------------

/// The table behind a result, if its rows can be edited.
#[tauri::command]
pub async fn editable_source(
    state: State<'_, AppState>,
    connection_id: String,
    database: String,
    dialect: Dialect,
    sql: String,
    result_columns: Vec<String>,
) -> CmdResult<EditSource> {
    let snap = snapshot(&state, &connection_id, &database).await?;
    no_panic("editing", || {
        sqlotus_core::editing::editable_source(&sql, dialect, &snap.catalog, &result_columns)
    })?
}

/// The statements that saving the edits would run, and any invalid values.
#[tauri::command]
pub fn edit_preview(source: EditSource, edits: Vec<RowEdit>, dialect: Dialect) -> CmdResult<EditPlan> {
    no_panic("editing", || {
        sqlotus_core::editing::edit_statements(&source, &edits, dialect)
    })
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct EditResult {
    applied: usize,
    committed: bool,
    /// Transactions left open (manual commit, or one the user had open).
    open_transactions: u32,
    notice: String,
}

/// Save edits on the tab's connection: all statements in one transaction,
/// each must change exactly the row it was meant for, else nothing is kept.
#[tauri::command]
pub async fn apply_edits(
    state: State<'_, AppState>,
    tab_id: String,
    source: EditSource,
    edits: Vec<RowEdit>,
) -> CmdResult<EditResult> {
    let session = state.session(&tab_id).ok_or("tab is not connected")?;
    if session.config.read_only {
        return Err("This connection is read-only, so the changes were not saved.".into());
    }
    let dialect = session.config.dialect;
    let plan = sqlotus_core::editing::edit_statements(&source, &edits, dialect);
    if !plan.problems.is_empty() {
        return Err(format!("Nothing was saved:\n{}", plan.problems.join("\n")));
    }
    let mut guard = session.conn.lock().await;
    crate::commands::ensure_connected(&session, &mut guard).await?;
    let conn = guard.as_mut().expect("connected");
    let limits = sqlotus_db::Limits::default();
    let was_open = conn.open_transactions().await.unwrap_or(0) > 0;
    if !was_open && conn.execute(conn.begin_statement(), &limits).await.failed {
        return Err("Could not start a transaction; nothing was saved.".into());
    }
    for (i, stmt) in plan.statements.iter().enumerate() {
        let out = conn.execute_counted(stmt, &limits).await;
        let problem = if out.failed {
            Some(
                out.messages
                    .iter()
                    .filter(|m| m.kind == sqlotus_db::MessageKind::Error)
                    .map(|m| m.text.clone())
                    .collect::<Vec<_>>()
                    .join("\n"),
            )
        } else if out.rows_affected == 0 {
            Some("the row was changed or deleted by someone else since it was loaded (0 rows matched)".to_string())
        } else {
            None
        };
        if let Some(p) = problem {
            if !was_open {
                let _ = conn.execute("ROLLBACK", &limits).await;
            }
            let msg = format!(
                "Change {} of {} failed: {p}\n{stmt}\n{}",
                i + 1,
                plan.statements.len(),
                if was_open {
                    "Your open transaction is still open; roll it back to undo the earlier changes."
                } else {
                    "Nothing was saved."
                }
            );
            return Err(msg);
        }
    }
    let keep_open = was_open || session.config.manual_commit;
    let mut committed = false;
    if !keep_open {
        let c = conn.execute("COMMIT", &limits).await;
        if c.failed {
            return Err(format!(
                "COMMIT failed: {}",
                c.messages.iter().map(|m| m.text.as_str()).collect::<Vec<_>>().join(" ")
            ));
        }
        committed = true;
    }
    let open = conn.open_transactions().await.unwrap_or(0);
    let n = plan.statements.len();
    Ok(EditResult {
        applied: n,
        committed,
        open_transactions: open,
        notice: if committed {
            format!("{n} change(s) saved.")
        } else {
            format!("{n} change(s) applied inside the open transaction; Commit to keep them.")
        },
    })
}
