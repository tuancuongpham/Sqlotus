//! Tauri commands called from the UI. Text offsets crossing this boundary
//! are UTF-16 code units (JavaScript string indices).

use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::{Arc, Mutex};

use serde::{Deserialize, Serialize};
use sqlq_core::catalog::ObjectKind;
use sqlq_core::search::{SearchHit, SearchOptions};
use sqlq_core::snippet::Snippet;
use sqlq_core::{CompletionResult, Dialect, byte_to_utf16, utf16_to_byte};
use sqlq_db::{BatchOutput, Connection, ConnectionConfig, Limits, load_catalog};
use sqlq_store::{HistoryEntry, NewHistory};
use tauri::State;

use crate::state::{AppState, Running, Session, Settings};

type CmdResult<T> = Result<T, String>;

fn err(e: impl std::fmt::Display) -> String {
    e.to_string()
}

/// Run editor-path code (completion, formatting, ...) so a bug in it becomes
/// an error for that one request instead of taking the whole app down.
fn no_panic<T>(what: &str, f: impl FnOnce() -> T) -> CmdResult<T> {
    std::panic::catch_unwind(std::panic::AssertUnwindSafe(f)).map_err(|p| {
        let msg = p
            .downcast_ref::<String>()
            .map(String::as_str)
            .or_else(|| p.downcast_ref::<&str>().copied())
            .unwrap_or("unknown");
        format!("internal error in {what}: {msg}")
    })
}

// ---------------------------------------------------------------------------
// settings
// ---------------------------------------------------------------------------

#[tauri::command]
pub fn get_settings(state: State<'_, AppState>) -> Settings {
    state.settings()
}

#[tauri::command]
pub fn save_settings(state: State<'_, AppState>, settings: Settings) -> CmdResult<()> {
    state.store.set_setting("settings", &settings).map_err(err)
}

#[tauri::command]
pub fn load_session(state: State<'_, AppState>) -> CmdResult<Option<serde_json::Value>> {
    state.store.get_setting("session").map_err(err)
}

#[tauri::command]
pub fn save_session(state: State<'_, AppState>, session: serde_json::Value) -> CmdResult<()> {
    state.store.set_setting("session", &session).map_err(err)
}

// ---------------------------------------------------------------------------
// connections
// ---------------------------------------------------------------------------

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SavedConnection {
    #[serde(flatten)]
    config: ConnectionConfig,
    has_password: bool,
}

#[tauri::command]
pub fn list_connections(state: State<'_, AppState>) -> CmdResult<Vec<SavedConnection>> {
    let list = state.store.list_connections().map_err(err)?;
    Ok(list
        .into_iter()
        .map(|config| {
            let has_password = state
                .store
                .get_password(&config.id)
                .ok()
                .flatten()
                .is_some();
            SavedConnection {
                config,
                has_password,
            }
        })
        .collect())
}

/// Save a connection. `password`: `Some` stores it, `None` keeps the stored
/// one, unless `forget_password` is set.
#[tauri::command]
pub fn save_connection(
    state: State<'_, AppState>,
    config: ConnectionConfig,
    password: Option<String>,
    forget_password: bool,
) -> CmdResult<()> {
    state.store.save_connection(&config).map_err(err)?;
    if forget_password {
        state.store.set_password(&config.id, None).map_err(err)?;
    } else if let Some(p) = password.filter(|p| !p.is_empty()) {
        state
            .store
            .set_password(&config.id, Some(&p))
            .map_err(err)?;
    }
    Ok(())
}

#[tauri::command]
pub fn delete_connection(state: State<'_, AppState>, id: String) -> CmdResult<()> {
    state.store.delete_connection(&id).map_err(err)
}

fn resolve_password(
    state: &AppState,
    config: &ConnectionConfig,
    password: Option<String>,
) -> Option<String> {
    password
        .filter(|p| !p.is_empty())
        .or_else(|| state.store.get_password(&config.id).ok().flatten())
}

/// Try a connection; returns the databases visible to it.
#[tauri::command]
pub async fn test_connection(
    state: State<'_, AppState>,
    config: ConnectionConfig,
    password: Option<String>,
) -> CmdResult<Vec<String>> {
    let password = resolve_password(&state, &config, password);
    let mut conn = Connection::open(&config, password.as_deref())
        .await
        .map_err(err)?;
    let dbs = conn.list_databases().await.map_err(err);
    conn.close().await;
    dbs
}

// ---------------------------------------------------------------------------
// tab sessions
// ---------------------------------------------------------------------------

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct TabConnection {
    database: String,
    databases: Vec<String>,
    dialect: Dialect,
}

#[tauri::command]
pub async fn connect_tab(
    state: State<'_, AppState>,
    tab_id: String,
    connection_id: String,
    database: Option<String>,
    password: Option<String>,
) -> CmdResult<TabConnection> {
    let mut config = state
        .store
        .get_connection(&connection_id)
        .map_err(err)?
        .ok_or_else(|| "connection not found".to_string())?;
    if let Some(db) = database.filter(|d| !d.is_empty()) {
        config.database = Some(db);
    }
    let password = resolve_password(&state, &config, password);
    let mut conn = Connection::open(&config, password.as_deref())
        .await
        .map_err(err)?;
    let current = conn.current_database().await.unwrap_or_default();
    let databases = conn.list_databases().await.unwrap_or_default();
    let dialect = config.dialect;
    let session = Arc::new(Session {
        config,
        password,
        conn: tokio::sync::Mutex::new(Some(conn)),
        database: Mutex::new(current.clone()),
        running: Mutex::new(None),
    });
    let old = state.sessions.lock().map_err(err)?.insert(tab_id, session);
    if let Some(old) = old {
        if let Some(c) = old.conn.lock().await.take() {
            c.close().await;
        }
    }
    Ok(TabConnection {
        database: current,
        databases,
        dialect,
    })
}

#[tauri::command]
pub async fn disconnect_tab(state: State<'_, AppState>, tab_id: String) -> CmdResult<()> {
    let old = state.sessions.lock().map_err(err)?.remove(&tab_id);
    if let Some(s) = old {
        if let Some(r) = s.running.lock().map_err(err)?.take() {
            r.abort.abort();
        }
        if let Ok(mut guard) = s.conn.try_lock() {
            if let Some(c) = guard.take() {
                c.close().await;
            }
        }
    }
    Ok(())
}

async fn ensure_connected(session: &Session, guard: &mut Option<Connection>) -> CmdResult<()> {
    if guard.is_none() {
        let mut config = session.config.clone();
        let db = session.database.lock().map_err(err)?.clone();
        if !db.is_empty() {
            config.database = Some(db);
        }
        *guard = Some(
            Connection::open(&config, session.password.as_deref())
                .await
                .map_err(err)?,
        );
    }
    Ok(())
}

#[tauri::command]
pub async fn change_database(
    state: State<'_, AppState>,
    tab_id: String,
    database: String,
) -> CmdResult<String> {
    let session = state.session(&tab_id).ok_or("tab is not connected")?;
    let mut guard = session.conn.lock().await;
    ensure_connected(&session, &mut guard).await?;
    let conn = guard.as_mut().expect("connected");
    if conn.dialect() == Dialect::Postgres {
        // Postgres cannot switch databases on a live connection
        let mut config = session.config.clone();
        config.database = Some(database.clone());
        let new = Connection::open(&config, session.password.as_deref())
            .await
            .map_err(err)?;
        if let Some(old) = guard.replace(new) {
            old.close().await;
        }
    } else {
        conn.use_database(&database).await.map_err(err)?;
    }
    *session.database.lock().map_err(err)? = database.clone();
    Ok(database)
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct BatchDto {
    /// UTF-16 offsets within the executed text.
    start: usize,
    end: usize,
    output: BatchOutput,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ExecuteResult {
    batches: Vec<BatchDto>,
    database: String,
    cancelled: bool,
}

/// Run `sql` on the tab's connection. Runs in its own task so `cancel_query`
/// can abort it.
#[tauri::command]
pub async fn execute(
    state: State<'_, AppState>,
    tab_id: String,
    sql: String,
) -> CmdResult<ExecuteResult> {
    let session = state.session(&tab_id).ok_or("tab is not connected")?;
    let limits = Limits {
        max_rows: state.settings().editor.max_rows.max(1),
        ..Limits::default()
    };
    let task_session = session.clone();
    let script = sql.clone();
    let (cancel_tx, cancel_rx) = tokio::sync::oneshot::channel();
    let handle = tauri::async_runtime::spawn(async move {
        let mut guard = task_session.conn.lock().await;
        ensure_connected(&task_session, &mut guard).await?;
        let conn = guard.as_mut().expect("connected");
        let _ = cancel_tx.send(conn.cancel_handle());
        let batches = sqlq_db::execute_script(conn, &script, &limits).await;
        // a USE statement may have switched database
        let db = conn.current_database().await.unwrap_or_default();
        Ok::<_, String>((batches, db))
    });
    if let Ok(cancel) = cancel_rx.await {
        *session.running.lock().map_err(err)? = Some(Running {
            abort: handle.inner().abort_handle(),
            cancel,
        });
    }
    let result = handle.await;
    session.running.lock().map_err(err)?.take();
    match result {
        Ok(Ok((batches, db))) => {
            if !db.is_empty() {
                *session.database.lock().map_err(err)? = db.clone();
            }
            let batches = batches
                .into_iter()
                .map(|b| BatchDto {
                    start: byte_to_utf16(&sql, b.start),
                    end: byte_to_utf16(&sql, b.end),
                    output: b.output,
                })
                .collect();
            Ok(ExecuteResult {
                batches,
                database: db,
                cancelled: false,
            })
        }
        Ok(Err(e)) => Err(e),
        Err(_) => {
            // aborted: the connection was dropped with the task; reconnect next time
            let db = session.database.lock().map_err(err)?.clone();
            Ok(ExecuteResult {
                batches: Vec::new(),
                database: db,
                cancelled: true,
            })
        }
    }
}

#[tauri::command]
pub async fn cancel_query(state: State<'_, AppState>, tab_id: String) -> CmdResult<()> {
    let Some(session) = state.session(&tab_id) else {
        return Ok(());
    };
    let running = session.running.lock().map_err(err)?.take();
    if let Some(r) = running {
        match r.cancel {
            sqlq_db::CancelHandle::None => r.abort.abort(),
            ref c => {
                // ask the server first; abort if it does not stop soon
                let _ = c.cancel().await;
                let abort = r.abort.clone();
                tauri::async_runtime::spawn(async move {
                    tokio::time::sleep(std::time::Duration::from_secs(3)).await;
                    abort.abort();
                });
            }
        }
    }
    Ok(())
}

// ---------------------------------------------------------------------------
// catalog, completion, formatting
// ---------------------------------------------------------------------------

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct CatalogInfo {
    objects: usize,
    definitions: usize,
    schemas: Vec<String>,
    tables: Vec<ObjectRef>,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ObjectRef {
    schema: String,
    name: String,
    kind: ObjectKind,
}

fn catalog_info(snap: &sqlq_db::CatalogSnapshot) -> CatalogInfo {
    CatalogInfo {
        objects: snap.catalog.objects.len(),
        definitions: snap.definitions.len(),
        schemas: snap.catalog.schemas.clone(),
        tables: snap
            .catalog
            .objects
            .iter()
            .map(|o| ObjectRef {
                schema: o.schema.clone(),
                name: o.name.clone(),
                kind: o.kind,
            })
            .collect(),
    }
}

async fn fetch_catalog(
    state: &AppState,
    connection_id: &str,
    database: &str,
) -> CmdResult<Arc<sqlq_db::CatalogSnapshot>> {
    let mut config = state
        .store
        .get_connection(connection_id)
        .map_err(err)?
        .ok_or("connection not found")?;
    if !database.is_empty() {
        config.database = Some(database.to_string());
    }
    // Reuse the password a connected tab already has (it may not be saved).
    let session_pw = state
        .sessions
        .lock()
        .map_err(err)?
        .values()
        .find(|s| s.config.id == connection_id)
        .and_then(|s| s.password.clone());
    let password = session_pw.or_else(|| state.store.get_password(connection_id).ok().flatten());
    let mut conn = Connection::open(&config, password.as_deref())
        .await
        .map_err(err)?;
    let snap = load_catalog(&mut conn).await.map_err(err);
    conn.close().await;
    Ok(state.put_catalog(connection_id, database, snap?))
}

/// Load (or reload) the catalog of a database for completion and search.
#[tauri::command]
pub async fn load_catalog_cmd(
    state: State<'_, AppState>,
    connection_id: String,
    database: String,
    refresh: bool,
) -> CmdResult<CatalogInfo> {
    if !refresh {
        if let Some(snap) = state.cached_catalog(&connection_id, &database) {
            return Ok(catalog_info(&snap));
        }
    }
    let snap = fetch_catalog(&state, &connection_id, &database).await?;
    Ok(catalog_info(&snap))
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CompleteArgs {
    connection_id: Option<String>,
    database: Option<String>,
    dialect: Dialect,
    text: String,
    offset: usize,
    server: Option<String>,
}

#[tauri::command]
pub fn complete(state: State<'_, AppState>, args: CompleteArgs) -> CmdResult<CompletionResult> {
    let catalog = match (&args.connection_id, &args.database) {
        (Some(c), Some(d)) => state.cached_catalog(c, d),
        _ => None,
    };
    let snippets = state.store.list_snippets().map_err(err)?;
    let mut opts = state.settings().completion;
    let user = std::env::var("USERNAME").unwrap_or_default();
    let machine = std::env::var("COMPUTERNAME").unwrap_or_default();
    opts.snippet_vars = HashMap::from([
        ("USER".to_string(), user),
        ("MACHINE".to_string(), machine),
        (
            "DBNAME".to_string(),
            args.database.clone().unwrap_or_default(),
        ),
        (
            "SERVER".to_string(),
            args.server.clone().unwrap_or_default(),
        ),
    ]);
    let offset = utf16_to_byte(&args.text, args.offset);
    let mut r = no_panic("completion", || {
        sqlq_core::complete(
            &args.text,
            offset,
            args.dialect,
            catalog.as_ref().map(|c| &c.catalog),
            &snippets,
            &opts,
        )
    })?;
    // byte -> UTF-16 for the editor
    r.replace_start = byte_to_utf16(&args.text, r.replace_start);
    r.replace_end = byte_to_utf16(&args.text, r.replace_end);
    for item in &mut r.items {
        if let Some(s) = item.replace_start {
            item.replace_start = Some(byte_to_utf16(&args.text, s));
        }
    }
    Ok(r)
}

#[tauri::command]
pub fn format_sql(state: State<'_, AppState>, text: String, dialect: Dialect) -> CmdResult<String> {
    let opts = state.settings().format;
    no_panic("formatter", || sqlq_core::format_sql(&text, dialect, &opts))
}

#[derive(Serialize)]
pub struct Range {
    start: usize,
    end: usize,
}

/// The statement around `offset` (for "execute current statement").
#[tauri::command]
pub fn statement_at(text: String, dialect: Dialect, offset: usize) -> CmdResult<Option<Range>> {
    let span = no_panic("statement split", || {
        sqlq_core::statements::statement_at(&text, dialect, utf16_to_byte(&text, offset))
    })?;
    Ok(span.map(|span| Range {
        start: byte_to_utf16(&text, span.start),
        end: byte_to_utf16(&text, span.end),
    }))
}

// ---------------------------------------------------------------------------
// SQL Search
// ---------------------------------------------------------------------------

#[tauri::command]
pub async fn search_objects(
    state: State<'_, AppState>,
    connection_id: String,
    databases: Vec<String>,
    query: String,
    options: SearchOptions,
) -> CmdResult<Vec<SearchHit>> {
    let mut hits = Vec::new();
    for db in databases {
        let snap = match state.cached_catalog(&connection_id, &db) {
            Some(s) => s,
            None => fetch_catalog(&state, &connection_id, &db).await?,
        };
        let name = if snap.catalog.database.is_empty() {
            db.clone()
        } else {
            snap.catalog.database.clone()
        };
        hits.extend(no_panic("search", || {
            sqlq_core::search::search(&query, &name, &snap.catalog, &snap.definitions, &options)
        })?);
    }
    hits.truncate(options.max_results.max(1));
    Ok(hits)
}

/// Script of an object: stored definition, or a CREATE TABLE built from
/// the catalog for tables.
#[tauri::command]
pub fn object_script(
    state: State<'_, AppState>,
    connection_id: String,
    database: String,
    schema: String,
    name: String,
    dialect: Dialect,
) -> CmdResult<String> {
    let snap = state
        .cached_catalog(&connection_id, &database)
        .ok_or("catalog not loaded")?;
    if let Some(d) = snap
        .definitions
        .iter()
        .find(|d| d.schema.eq_ignore_ascii_case(&schema) && d.name.eq_ignore_ascii_case(&name))
    {
        return Ok(d.definition.trim().to_string());
    }
    let obj = snap
        .catalog
        .find(Some(&schema), &name)
        .ok_or("object not found")?;
    let q = |s: &str| match dialect {
        Dialect::MsSql => format!("[{}]", s.replace(']', "]]")),
        _ => dialect.quote_ident(s),
    };
    let mut lines: Vec<String> = obj
        .columns
        .iter()
        .map(|c| {
            let mut l = format!("    {} {}", q(&c.name), c.data_type);
            if c.is_identity && dialect == Dialect::MsSql {
                l.push_str(" IDENTITY");
            }
            l.push_str(if c.nullable { " NULL" } else { " NOT NULL" });
            l
        })
        .collect();
    let pk: Vec<String> = obj
        .columns
        .iter()
        .filter(|c| c.is_primary_key)
        .map(|c| q(&c.name))
        .collect();
    if !pk.is_empty() {
        lines.push(format!("    PRIMARY KEY ({})", pk.join(", ")));
    }
    Ok(format!(
        "CREATE TABLE {}.{} (\n{}\n);",
        q(&obj.schema),
        q(&obj.name),
        lines.join(",\n")
    ))
}

// ---------------------------------------------------------------------------
// snippets
// ---------------------------------------------------------------------------

#[tauri::command]
pub fn list_snippets(state: State<'_, AppState>) -> CmdResult<Vec<Snippet>> {
    state.store.list_snippets().map_err(err)
}

#[tauri::command]
pub fn save_snippet(state: State<'_, AppState>, snippet: Snippet) -> CmdResult<()> {
    state.store.save_snippet(&snippet).map_err(err)
}

#[tauri::command]
pub fn delete_snippet(state: State<'_, AppState>, id: String) -> CmdResult<()> {
    state.store.delete_snippet(&id).map_err(err)
}

/// The newest `%LOCALAPPDATA%\Red Gate\SQL Prompt *\Snippets` folder.
pub fn redgate_snippets_dir() -> Option<PathBuf> {
    let base = PathBuf::from(std::env::var_os("LOCALAPPDATA")?).join("Red Gate");
    let mut dirs: Vec<PathBuf> = std::fs::read_dir(&base)
        .ok()?
        .filter_map(|e| e.ok().map(|e| e.path()))
        .filter(|p| {
            p.file_name()
                .and_then(|n| n.to_str())
                .is_some_and(|n| n.starts_with("SQL Prompt"))
        })
        .map(|p| p.join("Snippets"))
        .filter(|p| p.is_dir())
        .collect();
    dirs.sort();
    dirs.pop()
}

#[tauri::command]
pub fn import_redgate_snippets(
    state: State<'_, AppState>,
    dir: Option<String>,
) -> CmdResult<usize> {
    let dir = dir
        .map(PathBuf::from)
        .or_else(redgate_snippets_dir)
        .ok_or("Redgate snippets folder not found")?;
    state.store.import_redgate_snippets(&dir).map_err(err)
}

// ---------------------------------------------------------------------------
// history
// ---------------------------------------------------------------------------

#[tauri::command]
pub fn history_record(state: State<'_, AppState>, entry: NewHistory) -> CmdResult<i64> {
    state.store.record_history(&entry).map_err(err)
}

#[tauri::command]
pub fn history_search(
    state: State<'_, AppState>,
    query: String,
    limit: Option<usize>,
) -> CmdResult<Vec<HistoryEntry>> {
    state
        .store
        .search_history(&query, limit.unwrap_or(200))
        .map_err(err)
}

#[tauri::command]
pub fn history_tab(state: State<'_, AppState>, tab_id: String) -> CmdResult<Vec<HistoryEntry>> {
    state.store.tab_history(&tab_id).map_err(err)
}

#[tauri::command]
pub fn history_mark_closed(state: State<'_, AppState>, tab_id: String) -> CmdResult<()> {
    state.store.mark_tab_closed(&tab_id).map_err(err)
}

#[tauri::command]
pub fn history_recently_closed(
    state: State<'_, AppState>,
    limit: Option<usize>,
) -> CmdResult<Vec<HistoryEntry>> {
    state
        .store
        .recently_closed(limit.unwrap_or(20))
        .map_err(err)
}

#[tauri::command]
pub fn history_delete(state: State<'_, AppState>, id: i64) -> CmdResult<()> {
    state.store.delete_history(id).map_err(err)
}
