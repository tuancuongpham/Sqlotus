mod commands;
mod intel;
mod multi;
mod state;

use tauri::Manager;

use crate::state::AppState;

#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    tauri::Builder::default()
        .plugin(tauri_plugin_opener::init())
        .plugin(tauri_plugin_dialog::init())
        .setup(|app| {
            let path = app.path().app_data_dir()?.join("sqlq.db");
            let store = sqlq_store::Store::open(&path)?;
            // First run: the user's SQL Prompt snippets if present, else built-ins.
            if store.snippet_count()? == 0 {
                let imported = commands::redgate_snippets_dir()
                    .and_then(|dir| store.import_redgate_snippets(&dir).ok())
                    .unwrap_or(0);
                if imported == 0 {
                    for s in sqlq_core::snippet::builtin_snippets() {
                        store.save_snippet(&s)?;
                    }
                }
            }
            app.manage(AppState::new(store));
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
        ])
        .run(tauri::generate_context!())
        .expect("error while running tauri application");
}
