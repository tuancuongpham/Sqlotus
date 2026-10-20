//! Database tools beyond the editor: the ER diagram model and test data.

use std::sync::Arc;

use serde::Serialize;
use sqlotus_core::Dialect;
use sqlotus_core::catalog::{DbObject, ForeignKey, ObjectKind};
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
pub async fn diagram_model(state: State<'_, AppState>, connection_id: String, database: String) -> CmdResult<DiagramModel> {
    let snap = snapshot(&state, &connection_id, &database).await?;
    let tables: Vec<DbObject> = snap.catalog.objects.iter().filter(|o| o.kind == ObjectKind::Table).cloned().collect();
    // only relations between tables that are in the model
    let known = |schema: &str, name: &str| tables.iter().any(|t| t.schema.eq_ignore_ascii_case(schema) && t.name.eq_ignore_ascii_case(name));
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
        .find(|o| o.kind == ObjectKind::Table && o.schema.eq_ignore_ascii_case(schema) && o.name.eq_ignore_ascii_case(name))
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
    no_panic("test data", || testdata::plan_columns(table, &snap.catalog.foreign_keys, dialect))
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
        .filter(|f| f.from_columns.iter().any(|c| generator(c) == Some(Generator::ForeignKey)))
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
            let cols = f.to_columns.iter().map(|c| dialect.quote_ident(c)).collect::<Vec<_>>().join(", ");
            let not_null = f.to_columns.iter().map(|c| format!("{} IS NOT NULL", dialect.quote_ident(c))).collect::<Vec<_>>().join(" AND ");
            let from = qualified(dialect, &f.to_schema, &f.to_table);
            let sql = match dialect {
                Dialect::MsSql => format!("SELECT DISTINCT TOP (1000) {cols} FROM {from} WHERE {not_null}"),
                _ => format!("SELECT DISTINCT {cols} FROM {from} WHERE {not_null} LIMIT 1000"),
            };
            let rows = conn.query_strings(&sql).await.map_err(|e| format!("Could not read {from}: {e}"))?;
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
    no_panic("test data", || testdata::insert_script(&table, &plans, &fks, &ctx, &options, dialect))?
}
