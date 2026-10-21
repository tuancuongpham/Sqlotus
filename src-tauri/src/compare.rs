//! Schema compare between two databases (same or different saved
//! connections of the same kind) and the script that syncs the target.

use std::collections::HashMap;
use std::sync::Mutex;

use serde::{Deserialize, Serialize};
use sqlotus_core::Dialect;
use sqlotus_core::schemadiff::{self, CompareOptions, SchemaDiff, SchemaModel, SyncScript};
use tauri::State;

use crate::commands::{CmdResult, err, no_panic, open_database};
use crate::state::AppState;

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Endpoint {
    connection_id: String,
    database: String,
}

#[derive(Deserialize, Default)]
#[serde(default, rename_all = "camelCase")]
pub struct CompareRequestOptions {
    ignore_whitespace: Option<bool>,
    ignore_case: Option<bool>,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct CompareResult {
    id: String,
    dialect: Dialect,
    source: String,
    target: String,
    diff: SchemaDiff,
}

/// Comparisons kept for generating scripts (the latest few).
pub struct StoredCompare {
    source: SchemaModel,
    target: SchemaModel,
    diff: SchemaDiff,
    dialect: Dialect,
    options: CompareOptions,
}

#[derive(Default)]
pub struct CompareStore(Mutex<HashMap<String, StoredCompare>>);

async fn load(state: &AppState, e: &Endpoint, label: &str) -> CmdResult<SchemaModel> {
    load_model(state, &e.connection_id, &e.database, label).await
}

/// The schema of one database (for compare and source control).
pub(crate) async fn load_model(
    state: &AppState,
    connection_id: &str,
    database: &str,
    label: &str,
) -> CmdResult<SchemaModel> {
    let mut conn = open_database(state, connection_id, database)
        .await
        .map_err(|x| format!("Could not connect to the {label} ({database}): {x}"))?;
    let model = sqlotus_db::load_schema(&mut conn)
        .await
        .map_err(|x| format!("Could not read the {label} schema ({database}): {x}"));
    conn.close().await;
    model
}

/// Compare `source` with `target`: what would change in the target.
#[tauri::command]
pub async fn compare_schemas(
    state: State<'_, AppState>,
    store: State<'_, CompareStore>,
    source: Endpoint,
    target: Endpoint,
    options: Option<CompareRequestOptions>,
) -> CmdResult<CompareResult> {
    let cfg = |id: &str| {
        state
            .store
            .get_connection(id)
            .map_err(err)?
            .ok_or_else(|| "The connection no longer exists.".to_string())
    };
    let (src_cfg, tgt_cfg) = (cfg(&source.connection_id)?, cfg(&target.connection_id)?);
    if src_cfg.dialect != tgt_cfg.dialect {
        return Err(format!(
            "Both databases must be on the same kind of server (source is {:?}, target is {:?}).",
            src_cfg.dialect, tgt_cfg.dialect
        ));
    }
    let dialect = src_cfg.dialect;
    let (src, tgt) = tokio::join!(load(&state, &source, "source"), load(&state, &target, "target"));
    let (src, tgt) = (src?, tgt?);
    let o = options.unwrap_or_default();
    let options = CompareOptions {
        ignore_whitespace: o.ignore_whitespace.unwrap_or(true),
        ignore_case: o.ignore_case.unwrap_or(false),
        // Postgres keeps quoted names case-sensitive; SQL Server and MySQL (Windows) do not
        case_insensitive_names: dialect != Dialect::Postgres,
    };
    let diff = no_panic("schema compare", || schemadiff::compare(&src, &tgt, dialect, &options))?;
    let id = format!(
        "cmp-{}",
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_nanos())
            .unwrap_or_default()
    );
    {
        let mut map = store.0.lock().map_err(err)?;
        if map.len() >= 4 {
            map.clear(); // only recent comparisons are needed
        }
        map.insert(
            id.clone(),
            StoredCompare {
                source: src,
                target: tgt,
                diff: diff.clone(),
                dialect,
                options,
            },
        );
    }
    Ok(CompareResult {
        id,
        dialect,
        source: format!("{} / {}", src_cfg.name, source.database),
        target: format!("{} / {}", tgt_cfg.name, target.database),
        diff,
    })
}

/// A comparison set up once and opened again from the list.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SavedSide {
    connection_id: String,
    database: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SavedCompare {
    name: String,
    source: SavedSide,
    target: SavedSide,
    #[serde(default = "yes")]
    ignore_whitespace: bool,
    #[serde(default)]
    ignore_case: bool,
}

fn yes() -> bool {
    true
}

const SAVED_COMPARES: &str = "compare.saved";

#[tauri::command]
pub fn saved_compares(state: State<'_, AppState>) -> CmdResult<Vec<SavedCompare>> {
    Ok(state
        .store
        .get_setting::<Vec<SavedCompare>>(SAVED_COMPARES)
        .map_err(err)?
        .unwrap_or_default())
}

#[tauri::command]
pub fn save_saved_compares(state: State<'_, AppState>, list: Vec<SavedCompare>) -> CmdResult<()> {
    state.store.set_setting(SAVED_COMPARES, &list).map_err(err)
}

/// The script that applies the selected differences to the target.
#[tauri::command]
pub fn schema_sync_script(store: State<'_, CompareStore>, id: String, selected: Vec<String>) -> CmdResult<SyncScript> {
    let map = store.0.lock().map_err(err)?;
    let c = map
        .get(&id)
        .ok_or("This comparison is no longer available; compare again.")?;
    if selected.is_empty() {
        return Err("Select at least one difference.".into());
    }
    no_panic("schema compare", || {
        schemadiff::sync_script(&c.source, &c.target, &c.diff, &selected, c.dialect, &c.options)
    })
}
