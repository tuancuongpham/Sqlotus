//! Application state shared by commands: the store, one database session
//! per editor tab, and loaded catalogs.

use std::collections::HashMap;
use std::sync::{Arc, Mutex, RwLock};

use serde::{Deserialize, Serialize};
use sqlotus_core::lint::LintOptions;
use sqlotus_core::{CompletionOptions, FormatOptions};
use sqlotus_db::{CancelHandle, CatalogSnapshot, Connection, ConnectionConfig};
use sqlotus_store::Store;

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default, rename_all = "camelCase")]
pub struct EditorSettings {
    pub font_size: u32,
    /// Seconds between history autosaves of a changed tab.
    pub autosave_seconds: u32,
    pub max_rows: usize,
    /// `dark`, `light`, or `system` (follow Windows).
    pub theme: String,
    /// Execute (F5) runs the statement at the cursor when nothing is selected.
    pub f5_runs_statement: bool,
    /// Query history older than this many days is deleted; 0 keeps it all.
    pub history_days: u32,
}

impl Default for EditorSettings {
    fn default() -> Self {
        Self {
            font_size: 14,
            autosave_seconds: 15,
            max_rows: 100_000,
            theme: "dark".into(),
            f5_runs_statement: false,
            history_days: 0,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default, rename_all = "camelCase")]
pub struct SafetySettings {
    /// Scripts that only change rows run in a transaction; when they change
    /// more rows than this, Sqlotus asks before committing. 0 = never ask.
    pub confirm_rows_over: u64,
    /// Ask for values of variables the script uses without declaring them.
    pub prompt_variables: bool,
    /// Scripts that call procedures run in a transaction, and Sqlotus asks
    /// before committing what the procedures changed (it cannot count it).
    pub confirm_procedure_calls: bool,
    /// CSV files get a `'` before text values starting with `=`, `+`, `-`
    /// or `@`, so a spreadsheet does not run them as formulas.
    pub csv_formula_guard: bool,
}

impl Default for SafetySettings {
    fn default() -> Self {
        Self {
            confirm_rows_over: 1000,
            prompt_variables: true,
            confirm_procedure_calls: false,
            csv_formula_guard: true,
        }
    }
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default, rename_all = "camelCase")]
pub struct Settings {
    pub format: FormatOptions,
    pub completion: CompletionOptions,
    pub editor: EditorSettings,
    pub safety: SafetySettings,
    /// Code analysis rules that are switched off.
    pub analysis: LintOptions,
}

/// A tab's connection. The connection sits behind an async mutex because a
/// running query holds it; `None` means "reconnect before the next query"
/// (after a cancel dropped it).
pub struct Session {
    pub config: ConnectionConfig,
    pub password: Option<String>,
    pub conn: tokio::sync::Mutex<Option<Connection>>,
    pub database: Mutex<String>,
    pub running: Mutex<Option<Running>>,
}

pub struct Running {
    pub abort: tokio::task::AbortHandle,
    pub cancel: CancelHandle,
}

pub struct AppState {
    pub store: Arc<Store>,
    pub sessions: Mutex<HashMap<String, Arc<Session>>>,
    pub catalogs: RwLock<HashMap<String, Arc<CatalogSnapshot>>>,
    /// One-time message for the first window (e.g. data migrated from the old name).
    pub startup_notice: Mutex<Option<String>>,
    /// Activity monitor connections, per connection and database.
    pub monitors: tokio::sync::Mutex<HashMap<String, Connection>>,
    /// Database names per saved connection, recorded when a tab connects.
    pub server_databases: RwLock<HashMap<String, Vec<String>>>,
}

pub fn catalog_key(connection_id: &str, database: &str) -> String {
    format!("{connection_id}\u{1}{}", database.to_lowercase())
}

impl AppState {
    pub fn new(store: Store) -> Self {
        Self {
            store: Arc::new(store),
            sessions: Mutex::new(HashMap::new()),
            catalogs: RwLock::new(HashMap::new()),
            server_databases: RwLock::new(HashMap::new()),
            startup_notice: Mutex::new(None),
            monitors: tokio::sync::Mutex::new(HashMap::new()),
        }
    }

    pub fn session(&self, tab_id: &str) -> Option<Arc<Session>> {
        self.sessions.lock().ok()?.get(tab_id).cloned()
    }

    pub fn settings(&self) -> Settings {
        self.store.get_setting("settings").ok().flatten().unwrap_or_default()
    }

    /// Catalog from memory, falling back to the on-disk cache.
    pub fn cached_catalog(&self, connection_id: &str, database: &str) -> Option<Arc<CatalogSnapshot>> {
        let key = catalog_key(connection_id, database);
        if let Some(c) = self.catalogs.read().ok()?.get(&key) {
            return Some(c.clone());
        }
        let (snap, _) = self.store.load_catalog(connection_id, database).ok()??;
        let snap = Arc::new(snap);
        self.catalogs.write().ok()?.insert(key, snap.clone());
        Some(snap)
    }

    pub fn put_catalog(&self, connection_id: &str, database: &str, snap: CatalogSnapshot) -> Arc<CatalogSnapshot> {
        let _ = self.store.save_catalog(connection_id, database, &snap);
        let snap = Arc::new(snap);
        if let Ok(mut map) = self.catalogs.write() {
            map.insert(catalog_key(connection_id, database), snap.clone());
        }
        snap
    }
}
