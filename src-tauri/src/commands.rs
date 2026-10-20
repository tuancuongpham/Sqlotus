//! Tauri commands called from the UI. Text offsets crossing this boundary
//! are UTF-16 code units (JavaScript string indices).

use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::{Arc, Mutex};

use serde::{Deserialize, Serialize};
use sqlotus_core::catalog::ObjectKind;
use sqlotus_core::search::{SearchHit, SearchOptions};
use sqlotus_core::snippet::Snippet;
use sqlotus_core::{CompletionResult, Dialect, byte_to_utf16, utf16_to_byte};
use sqlotus_core::variables::VariableValue;
use sqlotus_db::{BatchOutput, Connection, ConnectionConfig, ExplainOutput, Limits, ScriptOptions, load_catalog};
use sqlotus_store::{HistoryEntry, NewHistory};
use tauri::State;

use crate::state::{AppState, Running, Session, Settings};

pub(crate) type CmdResult<T> = Result<T, String>;

pub(crate) fn err(e: impl std::fmt::Display) -> String {
    e.to_string()
}

/// Run editor-path code (completion, formatting, ...) so a bug in it becomes
/// an error for that one request instead of taking the whole app down.
pub(crate) fn no_panic<T>(what: &str, f: impl FnOnce() -> T) -> CmdResult<T> {
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

/// A one-time message for the first window, if any.
#[tauri::command]
pub fn take_startup_notice(state: State<'_, AppState>) -> Option<String> {
    state.startup_notice.lock().ok()?.take()
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
    if let Some(old) = old
        && let Some(c) = old.conn.lock().await.take()
    {
        c.close().await;
    }
    if let Ok(mut map) = state.server_databases.write() {
        map.insert(connection_id.clone(), databases.clone());
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
        if let Ok(mut guard) = s.conn.try_lock()
            && let Some(c) = guard.take()
        {
            c.close().await;
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
    pub(crate) start: usize,
    pub(crate) end: usize,
    pub(crate) output: BatchOutput,
}

/// A run whose changes wait in an open transaction for the user's decision.
#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct PendingCommit {
    /// Rows the run changed.
    pub(crate) rows: u64,
    /// The limit from the settings.
    pub(crate) limit: u64,
    /// Some statements failed, so the changes may be incomplete.
    pub(crate) failed: bool,
}

#[derive(Serialize, Default)]
#[serde(rename_all = "camelCase")]
pub struct ExecuteResult {
    pub(crate) batches: Vec<BatchDto>,
    pub(crate) database: String,
    pub(crate) cancelled: bool,
    /// Transactions left open after the run (0 when none or unknown).
    pub(crate) open_transactions: u32,
    /// Rows changed by INSERT/UPDATE/DELETE/MERGE, when the server reported it.
    pub(crate) rows_affected: Option<u64>,
    /// Set when Sqlotus wrapped the run in a transaction and needs a decision.
    pub(crate) pending_commit: Option<PendingCommit>,
    /// How Sqlotus handled the transaction, for the notification.
    pub(crate) notice: Option<String>,
}

/// How a run was wrapped in a transaction by Sqlotus.
#[derive(Clone, Copy, PartialEq, Eq)]
enum Wrapped {
    No,
    /// The connection is in manual-commit mode: leave it open.
    Manual,
    /// Row-count guard: commit unless too many rows changed.
    Guard,
}

fn uses_use_statement(sql: &str, dialect: Dialect) -> bool {
    sqlotus_core::tokenize(sql, dialect)
        .iter()
        .filter(|t| !t.is_trivia())
        .any(|t| t.kind == sqlotus_core::TokenKind::Word && t.upper() == "USE")
}

/// Run `sql` on the tab's connection. Runs in its own task so `cancel_query`
/// can abort it.
#[tauri::command]
pub async fn execute(
    state: State<'_, AppState>,
    tab_id: String,
    sql: String,
    variables: Option<Vec<VariableValue>>,
) -> CmdResult<ExecuteResult> {
    let session = state.session(&tab_id).ok_or("tab is not connected")?;
    let dialect = session.config.dialect;
    let changes = sqlotus_core::analysis::modifying_statements(&sql, dialect);
    if session.config.read_only && !changes.is_empty() {
        return Err(format!(
            "This connection is read-only, so the script was not run. It contains: {}. \
             Turn off \"Read-only\" in the connection settings to allow changes.",
            changes.join(", ")
        ));
    }
    let settings = state.settings();
    let limits = Limits {
        max_rows: settings.editor.max_rows.max(1),
        ..Limits::default()
    };
    let row_limit = settings.safety.confirm_rows_over;
    let rows_only = sqlotus_core::analysis::row_changes_only(&sql, dialect);
    let tx_control = sqlotus_core::analysis::has_transaction_control(&sql, dialect);
    let manual = session.config.manual_commit && !changes.is_empty() && !tx_control;
    let guard_wanted = !manual && row_limit > 0 && rows_only;
    let options = ScriptOptions {
        variables: variables.unwrap_or_default(),
        // SQL Server reports changed rows only through sp_executesql, whose
        // scope would undo a USE statement
        counted: rows_only && !uses_use_statement(&sql, dialect),
    };
    let task_session = session.clone();
    let script = sql.clone();
    let (cancel_tx, cancel_rx) = tokio::sync::oneshot::channel();
    let handle = tauri::async_runtime::spawn(async move {
        let mut guard = task_session.conn.lock().await;
        ensure_connected(&task_session, &mut guard).await?;
        let conn = guard.as_mut().expect("connected");
        let _ = cancel_tx.send(conn.cancel_handle());
        // wrap the run in a transaction unless one is already open
        let mut wrapped = Wrapped::No;
        if (manual || guard_wanted) && conn.open_transactions().await.unwrap_or(1) == 0 {
            let begin = conn.begin_statement();
            if !conn.execute(begin, &limits).await.failed {
                wrapped = if manual { Wrapped::Manual } else { Wrapped::Guard };
            }
        }
        let batches = sqlotus_db::execute_script_with(conn, &script, &limits, &options).await;
        let rows: u64 = batches.iter().map(|b| b.output.rows_affected).sum();
        let failed = batches.iter().any(|b| b.output.failed);
        let mut open = conn.open_transactions().await.unwrap_or(0);
        let mut pending = None;
        let mut notice = None;
        match wrapped {
            Wrapped::Guard if open > 0 => {
                if rows > row_limit || failed {
                    pending = Some(PendingCommit { rows, limit: row_limit, failed });
                } else {
                    let commit = conn.execute("COMMIT", &limits).await;
                    notice = Some(if commit.failed {
                        format!("COMMIT failed: {}", commit.messages.iter().map(|m| m.text.as_str()).collect::<Vec<_>>().join(" "))
                    } else {
                        format!("{rows} row(s) changed and committed.")
                    });
                    open = conn.open_transactions().await.unwrap_or(0);
                }
            }
            Wrapped::Manual => {
                notice = Some("Manual commit is on for this connection: the changes stay in an open transaction until you commit or roll back.".into());
            }
            _ => {}
        }
        // a USE statement may have switched database
        let db = conn.current_database().await.unwrap_or_default();
        let counted = options.counted || dialect != Dialect::MsSql;
        Ok::<_, String>((batches, db, open, (counted && rows_only).then_some(rows), pending, notice))
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
        Ok(Ok((batches, db, open_transactions, rows_affected, pending_commit, notice))) => {
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
                open_transactions,
                rows_affected,
                pending_commit,
                notice,
            })
        }
        Ok(Err(e)) => Err(e),
        Err(_) => {
            // aborted: the connection was dropped with the task; reconnect next time
            let db = session.database.lock().map_err(err)?.clone();
            Ok(ExecuteResult {
                database: db,
                cancelled: true,
                ..ExecuteResult::default()
            })
        }
    }
}

/// Estimated execution plans for `sql` (nothing is executed).
#[tauri::command]
pub async fn explain(state: State<'_, AppState>, tab_id: String, sql: String) -> CmdResult<ExplainOutput> {
    let session = state.session(&tab_id).ok_or("tab is not connected")?;
    let mut guard = session.conn.lock().await;
    ensure_connected(&session, &mut guard).await?;
    let conn = guard.as_mut().expect("connected");
    Ok(conn.explain(&sql).await)
}

/// Variables `sql` uses without declaring them (to ask for their values).
#[tauri::command]
pub fn undeclared_variables(text: String, dialect: Dialect) -> CmdResult<Vec<String>> {
    no_panic("variables", || sqlotus_core::variables::undeclared_variables(&text, dialect))
}

#[tauri::command]
pub async fn cancel_query(state: State<'_, AppState>, tab_id: String) -> CmdResult<()> {
    let Some(session) = state.session(&tab_id) else {
        return Ok(());
    };
    let running = session.running.lock().map_err(err)?.take();
    if let Some(r) = running {
        match r.cancel {
            sqlotus_db::CancelHandle::None => r.abort.abort(),
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

fn catalog_info(snap: &sqlotus_db::CatalogSnapshot) -> CatalogInfo {
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

pub(crate) async fn fetch_catalog(
    state: &AppState,
    connection_id: &str,
    database: &str,
) -> CmdResult<Arc<sqlotus_db::CatalogSnapshot>> {
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
    if !refresh && let Some(snap) = state.cached_catalog(&connection_id, &database) {
        return Ok(catalog_info(&snap));
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

/// Other databases named as `Db.` qualifiers in the text.
fn referenced_databases(
    text: &str,
    dialect: Dialect,
    databases: &[String],
    current: &str,
) -> Vec<String> {
    let tokens: Vec<_> = sqlotus_core::tokenize(text, dialect)
        .into_iter()
        .filter(|t| !t.is_trivia())
        .collect();
    let mut found: Vec<String> = Vec::new();
    for w in tokens.windows(2) {
        if w[0].is_name() && w[1].kind == sqlotus_core::TokenKind::Dot {
            let name = w[0].ident_value();
            if let Some(db) = databases
                .iter()
                .find(|d| d.eq_ignore_ascii_case(&name) && !d.eq_ignore_ascii_case(current))
                && !found.contains(db)
            {
                found.push(db.clone());
            }
        }
    }
    found
}

#[tauri::command]
pub async fn complete(
    state: State<'_, AppState>,
    args: CompleteArgs,
) -> CmdResult<CompletionResult> {
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
    // three-part names: load the catalogs of other databases the text mentions
    let mut others = Vec::new();
    if let (Some(conn_id), Some(current)) = (&args.connection_id, &args.database) {
        let databases = state
            .server_databases
            .read()
            .ok()
            .and_then(|m| m.get(conn_id).cloned())
            .unwrap_or_default();
        for db in referenced_databases(&args.text, args.dialect, &databases, current) {
            let snap = match state.cached_catalog(conn_id, &db) {
                Some(s) => Some(s),
                None => fetch_catalog(&state, conn_id, &db).await.ok(),
            };
            others.extend(snap);
        }
        opts.databases = databases;
    }
    let other_catalogs: Vec<&sqlotus_core::Catalog> = others.iter().map(|s| &s.catalog).collect();
    let offset = utf16_to_byte(&args.text, args.offset);
    let mut r = no_panic("completion", || {
        sqlotus_core::completion::complete_with_others(
            &args.text,
            offset,
            args.dialect,
            catalog.as_ref().map(|c| &c.catalog),
            &other_catalogs,
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
    no_panic("formatter", || {
        sqlotus_core::format_sql(&text, dialect, &opts)
    })
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
        sqlotus_core::statements::statement_at(&text, dialect, utf16_to_byte(&text, offset))
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
            sqlotus_core::search::search(&query, &name, &snap.catalog, &snap.definitions, &options)
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
    Ok(sqlotus_core::script::create_table_script(obj, dialect))
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
