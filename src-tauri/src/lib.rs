mod commands;
mod doctor;
mod excel;
mod intel;
mod multi;
mod state;
mod tools;

use tauri::Manager;

use crate::state::AppState;

/// Copy the database of the app's previous name (SqlSupportQuery) on the
/// first start of Sqlotus, so connections, history and snippets carry over.
fn migrate_legacy_data(dir: &std::path::Path, path: &std::path::Path) -> bool {
    if path.exists() {
        return false;
    }
    let Some(old_dir) = dir.parent().map(|p| p.join("com.sqlsupportquery.app")) else {
        return false;
    };
    let old = old_dir.join("sqlq.db");
    if !old.exists() || std::fs::create_dir_all(dir).is_err() {
        return false;
    }
    // the database and its write-ahead log belong together
    for suffix in ["", "-wal", "-shm"] {
        let from = old_dir.join(format!("sqlq.db{suffix}"));
        if from.exists() && std::fs::copy(&from, dir.join(format!("sqlotus.db{suffix}"))).is_err() {
            return false;
        }
    }
    true
}

#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    tauri::Builder::default()
        .plugin(tauri_plugin_opener::init())
        .plugin(tauri_plugin_dialog::init())
        .setup(|app| {
            let dir = app.path().app_data_dir()?;
            let path = dir.join("sqlotus.db");
            let migrated = migrate_legacy_data(&dir, &path);
            let store = sqlotus_store::Store::open(&path)?;
            let notice = migrated.then(|| {
                "Your connections, query history and snippets from SqlSupportQuery were moved over.".to_string()
            });
            // First run: the user's SQL Prompt snippets if present, else built-ins.
            if store.snippet_count()? == 0 {
                let imported = commands::redgate_snippets_dir()
                    .and_then(|dir| store.import_redgate_snippets(&dir).ok())
                    .unwrap_or(0);
                if imported == 0 {
                    for s in sqlotus_core::snippet::builtin_snippets() {
                        store.save_snippet(&s)?;
                    }
                }
            }
            let state = AppState::new(store);
            if let Ok(mut n) = state.startup_notice.lock() {
                *n = notice;
            }
            app.manage(state);
            Ok(())
        })
        .invoke_handler(tauri::generate_handler![
            commands::get_settings,
            commands::save_settings,
            commands::load_session,
            commands::save_session,
            commands::list_connections,
            commands::save_connection,
            commands::delete_connection,
            commands::test_connection,
            commands::connect_tab,
            commands::disconnect_tab,
            commands::change_database,
            commands::execute,
            commands::cancel_query,
            commands::load_catalog_cmd,
            commands::complete,
            commands::format_sql,
            commands::statement_at,
            commands::search_objects,
            commands::object_script,
            commands::list_snippets,
            commands::save_snippet,
            commands::delete_snippet,
            commands::import_redgate_snippets,
            commands::history_record,
            commands::history_search,
            commands::history_tab,
            commands::history_mark_closed,
            commands::history_recently_closed,
            commands::history_delete,
            intel::diagnostics,
            intel::execution_warnings,
            intel::hover,
            intel::signature_help,
            intel::object_at,
            intel::refactor_text,
            intel::catalog_objects,
            intel::script_object,
            intel::rename_target,
            intel::find_invalid_objects,
            intel::save_text_file,
            multi::execute_multi,
            doctor::diagnose_connection,
            doctor::apply_fix,
            commands::explain,
            commands::undeclared_variables,
            excel::export_xlsx,
            tools::diagram_model,
            tools::test_data_plan,
            tools::test_data_script,
            commands::take_startup_notice,
        ])
        .run(tauri::generate_context!())
        .expect("error while running tauri application");
}
