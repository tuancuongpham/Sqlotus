mod activity;
mod backup;
mod commands;
mod compare;
mod doctor;
mod excel;
mod health;
mod intel;
mod multi;
mod source_control;
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
            // First run: a few built-in snippets.
            if store.snippet_count()? == 0 {
                for s in sqlotus_core::snippet::builtin_snippets() {
                    store.save_snippet(&s)?;
                }
            }
            let state = AppState::new(store);
            if let Ok(mut n) = state.startup_notice.lock() {
                *n = notice;
            }
            // query history kept for a limited time (Options → Editor), pruned
            // in the background: a large history must not hold up the window
            let days = state.settings().editor.history_days;
            if days > 0 {
                let store = state.store.clone();
                std::thread::spawn(move || {
                    let _ = store.prune_history(days);
                });
            }
            app.manage(state);
            app.manage(compare::CompareStore::default());
            app.manage(source_control::SourceControlStore::default());
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
            commands::import_snippets,
            commands::history_record,
            commands::history_search,
            commands::history_tab,
            commands::history_mark_closed,
            commands::history_recently_closed,
            commands::history_delete,
            commands::history_prune,
            intel::diagnostics,
            intel::analysis_rules,
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
            tools::object_references,
            tools::modifying_statements,
            tools::editable_source,
            tools::edit_preview,
            tools::apply_edits,
            tools::result_links,
            tools::link_query,
            tools::table_data_script,
            tools::find_value_script,
            backup::backup_info,
            backup::backup_contents,
            backup::restore_moves,
            backup::backup_script,
            backup::restore_script,
            health::index_health,
            activity::server_activity,
            activity::kill_session,
            activity::close_activity,
            compare::compare_schemas,
            compare::schema_sync_script,
            compare::saved_compares,
            compare::save_saved_compares,
            source_control::sc_link_info,
            source_control::sc_link,
            source_control::sc_unlink,
            source_control::sc_status,
            source_control::sc_commit,
            source_control::sc_commit_files,
            source_control::sc_get_latest_script,
            source_control::sc_git,
            commands::take_startup_notice,
        ])
        .run(tauri::generate_context!())
        .expect("error while running tauri application");
}
