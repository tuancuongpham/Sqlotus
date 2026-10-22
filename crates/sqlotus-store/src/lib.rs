//! Local persistence in one SQLite file: saved connections, settings,
//! snippets, query history (full-text searchable) and a catalog cache.
//! Passwords go to the OS credential store, never into the database.

use std::path::Path;
use std::sync::Mutex;
use std::time::{SystemTime, UNIX_EPOCH};

use rusqlite::{Connection, OptionalExtension, params};
use serde::de::DeserializeOwned;
use serde::{Deserialize, Serialize};
use sqlotus_core::snippet::Snippet;
use sqlotus_db::{CatalogSnapshot, ConnectionConfig};

#[derive(Debug, thiserror::Error)]
pub enum StoreError {
    #[error("database: {0}")]
    Sql(#[from] rusqlite::Error),
    #[error("json: {0}")]
    Json(#[from] serde_json::Error),
    #[error("io: {0}")]
    Io(#[from] std::io::Error),
    #[error("credential store: {0}")]
    Keyring(String),
}

pub type Result<T, E = StoreError> = std::result::Result<T, E>;

const KEYRING_SERVICE: &str = "Sqlotus";
/// Credential-store service used before the rename to Sqlotus.
const LEGACY_KEYRING_SERVICE: &str = "SqlSupportQuery";
/// Autosaves of the same tab within this window update one history entry.
const AUTOSAVE_COALESCE_MS: i64 = 10 * 60 * 1000;

pub fn now_ms() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_millis() as i64)
        .unwrap_or(0)
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum HistoryKind {
    Autosave,
    Execute,
}

impl HistoryKind {
    fn as_str(self) -> &'static str {
        match self {
            HistoryKind::Autosave => "autosave",
            HistoryKind::Execute => "execute",
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct NewHistory {
    pub tab_id: String,
    pub title: String,
    pub connection_name: Option<String>,
    pub database: Option<String>,
    pub dialect: String,
    pub text: String,
    pub kind: HistoryKind,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct HistoryEntry {
    pub id: i64,
    pub tab_id: String,
    pub title: String,
    pub connection_name: Option<String>,
    pub database: Option<String>,
    pub dialect: String,
    pub text: String,
    pub kind: String,
    pub created_at: i64,
    pub updated_at: i64,
    pub closed: bool,
}

pub struct Store {
    db: Mutex<Connection>,
}

const SCHEMA: &str = r#"
CREATE TABLE IF NOT EXISTS connections (
    id TEXT PRIMARY KEY,
    json TEXT NOT NULL,
    sort INTEGER NOT NULL DEFAULT 0
);
CREATE TABLE IF NOT EXISTS settings (
    key TEXT PRIMARY KEY,
    value TEXT NOT NULL
);
CREATE TABLE IF NOT EXISTS snippets (
    id TEXT PRIMARY KEY,
    prefix TEXT NOT NULL,
    json TEXT NOT NULL
);
CREATE TABLE IF NOT EXISTS history (
    id INTEGER PRIMARY KEY,
    tab_id TEXT NOT NULL,
    title TEXT NOT NULL,
    connection_name TEXT,
    database TEXT,
    dialect TEXT NOT NULL,
    text TEXT NOT NULL,
    kind TEXT NOT NULL,
    created_at INTEGER NOT NULL,
    updated_at INTEGER NOT NULL,
    closed INTEGER NOT NULL DEFAULT 0
);
CREATE INDEX IF NOT EXISTS history_tab ON history(tab_id, id);
CREATE INDEX IF NOT EXISTS history_updated ON history(updated_at);
CREATE VIRTUAL TABLE IF NOT EXISTS history_fts USING fts5(
    text, title, content='history', content_rowid='id', tokenize='trigram'
);
CREATE TRIGGER IF NOT EXISTS history_ai AFTER INSERT ON history BEGIN
    INSERT INTO history_fts(rowid, text, title) VALUES (new.id, new.text, new.title);
END;
CREATE TRIGGER IF NOT EXISTS history_ad AFTER DELETE ON history BEGIN
    INSERT INTO history_fts(history_fts, rowid, text, title) VALUES ('delete', old.id, old.text, old.title);
END;
CREATE TRIGGER IF NOT EXISTS history_au AFTER UPDATE OF text, title ON history BEGIN
    INSERT INTO history_fts(history_fts, rowid, text, title) VALUES ('delete', old.id, old.text, old.title);
    INSERT INTO history_fts(rowid, text, title) VALUES (new.id, new.text, new.title);
END;
CREATE TABLE IF NOT EXISTS catalog_cache (
    connection_id TEXT NOT NULL,
    database TEXT NOT NULL,
    json TEXT NOT NULL,
    loaded_at INTEGER NOT NULL,
    PRIMARY KEY (connection_id, database)
);
"#;

fn history_row(r: &rusqlite::Row<'_>) -> rusqlite::Result<HistoryEntry> {
    Ok(HistoryEntry {
        id: r.get(0)?,
        tab_id: r.get(1)?,
        title: r.get(2)?,
        connection_name: r.get(3)?,
        database: r.get(4)?,
        dialect: r.get(5)?,
        text: r.get(6)?,
        kind: r.get(7)?,
        created_at: r.get(8)?,
        updated_at: r.get(9)?,
        closed: r.get::<_, i64>(10)? != 0,
    })
}

/// The JSON values in the first column of `sql`'s rows. A row this version
/// cannot read (written by a newer one, with a kind of server or option it
/// does not know) is left out and left alone, rather than hiding every other
/// row behind an error.
fn json_rows<T: DeserializeOwned>(db: &Connection, sql: &str) -> Result<Vec<T>> {
    let mut stmt = db.prepare(sql)?;
    let rows = stmt.query_map([], |r| r.get::<_, String>(0))?;
    let mut out = Vec::new();
    for json in rows {
        if let Ok(v) = serde_json::from_str(&json?) {
            out.push(v);
        }
    }
    Ok(out)
}

const HISTORY_COLS: &str = "h.id, h.tab_id, h.title, h.connection_name, h.database, h.dialect, h.text, h.kind, h.created_at, h.updated_at, h.closed";

impl Store {
    pub fn open(path: &Path) -> Result<Self> {
        if let Some(dir) = path.parent() {
            std::fs::create_dir_all(dir)?;
        }
        Self::init(Connection::open(path)?)
    }

    pub fn open_in_memory() -> Result<Self> {
        Self::init(Connection::open_in_memory()?)
    }

    fn init(conn: Connection) -> Result<Self> {
        conn.pragma_update(None, "journal_mode", "WAL")?;
        conn.pragma_update(None, "foreign_keys", "ON")?;
        conn.execute_batch(SCHEMA)?;
        Ok(Self { db: Mutex::new(conn) })
    }

    fn with<T>(&self, f: impl FnOnce(&Connection) -> Result<T>) -> Result<T> {
        let guard = self.db.lock().unwrap_or_else(|e| e.into_inner());
        f(&guard)
    }

    // -- connections ------------------------------------------------------

    pub fn list_connections(&self) -> Result<Vec<ConnectionConfig>> {
        self.with(|db| json_rows(db, "SELECT json FROM connections ORDER BY sort, rowid"))
    }

    pub fn get_connection(&self, id: &str) -> Result<Option<ConnectionConfig>> {
        self.with(|db| {
            let json: Option<String> = db
                .query_row("SELECT json FROM connections WHERE id = ?1", [id], |r| r.get(0))
                .optional()?;
            Ok(json.map(|j| serde_json::from_str(&j)).transpose()?)
        })
    }

    pub fn save_connection(&self, cfg: &ConnectionConfig) -> Result<()> {
        let json = serde_json::to_string(cfg)?;
        self.with(|db| {
            db.execute(
                "INSERT INTO connections (id, json, sort) VALUES (?1, ?2, (SELECT COALESCE(MAX(sort), 0) + 1 FROM connections))
                 ON CONFLICT(id) DO UPDATE SET json = excluded.json",
                params![cfg.id, json],
            )?;
            Ok(())
        })
    }

    pub fn delete_connection(&self, id: &str) -> Result<()> {
        self.with(|db| {
            db.execute("DELETE FROM connections WHERE id = ?1", [id])?;
            db.execute("DELETE FROM catalog_cache WHERE connection_id = ?1", [id])?;
            Ok(())
        })?;
        self.set_password(id, None)
    }

    // -- passwords --------------------------------------------------------

    pub fn set_password(&self, connection_id: &str, password: Option<&str>) -> Result<()> {
        let entry =
            keyring::Entry::new(KEYRING_SERVICE, connection_id).map_err(|e| StoreError::Keyring(e.to_string()))?;
        match password {
            Some(p) => entry.set_password(p).map_err(|e| StoreError::Keyring(e.to_string())),
            None => match entry.delete_credential() {
                Ok(()) | Err(keyring::Error::NoEntry) => Ok(()),
                Err(e) => Err(StoreError::Keyring(e.to_string())),
            },
        }
    }

    pub fn get_password(&self, connection_id: &str) -> Result<Option<String>> {
        let read = |service: &str| -> Result<Option<String>> {
            let entry = keyring::Entry::new(service, connection_id).map_err(|e| StoreError::Keyring(e.to_string()))?;
            match entry.get_password() {
                Ok(p) => Ok(Some(p)),
                Err(keyring::Error::NoEntry) => Ok(None),
                Err(e) => Err(StoreError::Keyring(e.to_string())),
            }
        };
        if let Some(p) = read(KEYRING_SERVICE)? {
            return Ok(Some(p));
        }
        // saved before the rename: move it under the new service name
        let legacy = read(LEGACY_KEYRING_SERVICE)?;
        if let Some(p) = &legacy {
            self.set_password(connection_id, Some(p))?;
            if let Ok(old) = keyring::Entry::new(LEGACY_KEYRING_SERVICE, connection_id) {
                let _ = old.delete_credential();
            }
        }
        Ok(legacy)
    }

    // -- settings ---------------------------------------------------------

    pub fn get_setting<T: DeserializeOwned>(&self, key: &str) -> Result<Option<T>> {
        self.with(|db| {
            let v: Option<String> = db
                .query_row("SELECT value FROM settings WHERE key = ?1", [key], |r| r.get(0))
                .optional()?;
            Ok(v.map(|v| serde_json::from_str(&v)).transpose()?)
        })
    }

    pub fn set_setting<T: Serialize + ?Sized>(&self, key: &str, value: &T) -> Result<()> {
        let json = serde_json::to_string(value)?;
        self.with(|db| {
            db.execute(
                "INSERT INTO settings (key, value) VALUES (?1, ?2) ON CONFLICT(key) DO UPDATE SET value = excluded.value",
                params![key, json],
            )?;
            Ok(())
        })
    }

    // -- snippets ---------------------------------------------------------

    pub fn list_snippets(&self) -> Result<Vec<Snippet>> {
        self.with(|db| json_rows(db, "SELECT json FROM snippets ORDER BY prefix"))
    }

    pub fn save_snippet(&self, s: &Snippet) -> Result<()> {
        let json = serde_json::to_string(s)?;
        self.with(|db| {
            db.execute(
                "INSERT INTO snippets (id, prefix, json) VALUES (?1, ?2, ?3)
                 ON CONFLICT(id) DO UPDATE SET prefix = excluded.prefix, json = excluded.json",
                params![s.id, s.prefix, json],
            )?;
            Ok(())
        })
    }

    pub fn delete_snippet(&self, id: &str) -> Result<()> {
        self.with(|db| {
            db.execute("DELETE FROM snippets WHERE id = ?1", [id])?;
            Ok(())
        })
    }

    pub fn snippet_count(&self) -> Result<i64> {
        self.with(|db| Ok(db.query_row("SELECT COUNT(*) FROM snippets", [], |r| r.get(0))?))
    }

    /// Import every `*.json` snippet file of a folder (`prefix`,
    /// `description`, `body`, `placeholders`). Snippets with the same id are
    /// replaced. Returns how many were imported.
    pub fn import_snippets(&self, dir: &Path) -> Result<usize> {
        let mut count = 0;
        for entry in std::fs::read_dir(dir)? {
            let path = entry?.path();
            if path.extension().and_then(|e| e.to_str()) != Some("json") {
                continue;
            }
            let Ok(text) = std::fs::read_to_string(&path) else {
                continue;
            };
            let Ok(mut snippet) = serde_json::from_str::<Snippet>(text.trim_start_matches('\u{feff}')) else {
                continue;
            };
            if snippet.id.is_empty() {
                snippet.id = path
                    .file_stem()
                    .and_then(|s| s.to_str())
                    .unwrap_or_default()
                    .to_string();
            }
            self.save_snippet(&snippet)?;
            count += 1;
        }
        Ok(count)
    }

    // -- history ----------------------------------------------------------

    /// Record the text of a tab. Identical text only refreshes the
    /// timestamp; autosaves shortly after another autosave update it in
    /// place so history keeps meaningful versions rather than keystrokes.
    pub fn record_history(&self, h: &NewHistory) -> Result<i64> {
        if h.text.trim().is_empty() {
            return Ok(0);
        }
        let now = now_ms();
        self.with(|db| {
            let last: Option<(i64, String, String, i64)> = db
                .query_row(
                    "SELECT id, text, kind, created_at FROM history WHERE tab_id = ?1 ORDER BY id DESC LIMIT 1",
                    [&h.tab_id],
                    |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?)),
                )
                .optional()?;
            if let Some((id, text, kind, created)) = last {
                if text == h.text {
                    let kind = if h.kind == HistoryKind::Execute { "execute" } else { kind.as_str() };
                    db.execute(
                        "UPDATE history SET updated_at = ?2, kind = ?3, title = ?4, connection_name = ?5, database = ?6, closed = 0 WHERE id = ?1",
                        params![id, now, kind, h.title, h.connection_name, h.database],
                    )?;
                    return Ok(id);
                }
                if h.kind == HistoryKind::Autosave && kind == "autosave" && now - created < AUTOSAVE_COALESCE_MS {
                    db.execute(
                        "UPDATE history SET text = ?2, updated_at = ?3, title = ?4, connection_name = ?5, database = ?6, closed = 0 WHERE id = ?1",
                        params![id, h.text, now, h.title, h.connection_name, h.database],
                    )?;
                    return Ok(id);
                }
            }
            db.execute(
                "INSERT INTO history (tab_id, title, connection_name, database, dialect, text, kind, created_at, updated_at)
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?8)",
                params![h.tab_id, h.title, h.connection_name, h.database, h.dialect, h.text, h.kind.as_str(), now],
            )?;
            Ok(db.last_insert_rowid())
        })
    }

    /// Search history text and titles; every whitespace-separated term must
    /// match (substring). An empty query lists the most recent entries.
    pub fn search_history(&self, query: &str, limit: usize) -> Result<Vec<HistoryEntry>> {
        let terms: Vec<&str> = query.split_whitespace().collect();
        self.with(|db| {
            let limit = limit as i64;
            let mut out = Vec::new();
            if terms.is_empty() {
                let mut stmt = db.prepare(&format!(
                    "SELECT {HISTORY_COLS} FROM history h ORDER BY h.updated_at DESC LIMIT ?1"
                ))?;
                for r in stmt.query_map([limit], history_row)? {
                    out.push(r?);
                }
            } else if terms.iter().all(|t| t.chars().count() >= 3) {
                // trigram index: each term as a quoted substring phrase
                let expr = terms
                    .iter()
                    .map(|t| format!("\"{}\"", t.replace('"', "\"\"")))
                    .collect::<Vec<_>>()
                    .join(" AND ");
                let mut stmt = db.prepare(&format!(
                    "SELECT {HISTORY_COLS} FROM history_fts f JOIN history h ON h.id = f.rowid
                     WHERE history_fts MATCH ?1 ORDER BY h.updated_at DESC LIMIT ?2"
                ))?;
                for r in stmt.query_map(params![expr, limit], history_row)? {
                    out.push(r?);
                }
            } else {
                let mut sql = format!("SELECT {HISTORY_COLS} FROM history h WHERE 1 = 1");
                for i in 0..terms.len() {
                    sql.push_str(&format!(
                        " AND (h.text LIKE ?{n} ESCAPE '\\' OR h.title LIKE ?{n} ESCAPE '\\')",
                        n = i + 1
                    ));
                }
                sql.push_str(&format!(" ORDER BY h.updated_at DESC LIMIT {limit}"));
                let patterns: Vec<String> = terms
                    .iter()
                    .map(|t| format!("%{}%", t.replace('\\', "\\\\").replace('%', "\\%").replace('_', "\\_")))
                    .collect();
                let mut stmt = db.prepare(&sql)?;
                for r in stmt.query_map(rusqlite::params_from_iter(patterns.iter()), history_row)? {
                    out.push(r?);
                }
            }
            Ok(out)
        })
    }

    pub fn get_history(&self, id: i64) -> Result<Option<HistoryEntry>> {
        self.with(|db| {
            Ok(db
                .query_row(
                    &format!("SELECT {HISTORY_COLS} FROM history h WHERE h.id = ?1"),
                    [id],
                    history_row,
                )
                .optional()?)
        })
    }

    /// All versions of one tab, newest first.
    pub fn tab_history(&self, tab_id: &str) -> Result<Vec<HistoryEntry>> {
        self.with(|db| {
            let mut stmt = db.prepare(&format!(
                "SELECT {HISTORY_COLS} FROM history h WHERE h.tab_id = ?1 ORDER BY h.id DESC"
            ))?;
            let rows = stmt.query_map([tab_id], history_row)?;
            Ok(rows.collect::<rusqlite::Result<Vec<_>>>()?)
        })
    }

    pub fn mark_tab_closed(&self, tab_id: &str) -> Result<()> {
        self.with(|db| {
            db.execute(
                "UPDATE history SET closed = 1 WHERE id = (SELECT MAX(id) FROM history WHERE tab_id = ?1)",
                [tab_id],
            )?;
            Ok(())
        })
    }

    /// Latest version of recently closed tabs, newest first.
    pub fn recently_closed(&self, limit: usize) -> Result<Vec<HistoryEntry>> {
        self.with(|db| {
            let mut stmt = db.prepare(&format!(
                "SELECT {HISTORY_COLS} FROM history h WHERE h.closed = 1 ORDER BY h.updated_at DESC LIMIT ?1"
            ))?;
            let rows = stmt.query_map([limit as i64], history_row)?;
            Ok(rows.collect::<rusqlite::Result<Vec<_>>>()?)
        })
    }

    pub fn delete_history(&self, id: i64) -> Result<()> {
        self.with(|db| {
            db.execute("DELETE FROM history WHERE id = ?1", [id])?;
            Ok(())
        })
    }

    /// Delete history not touched for `days` days, except the latest
    /// version of each tab that is still open. Returns how many versions
    /// were deleted. A few hundred at a time: deleting a year of history
    /// takes seconds, and other calls wait for the store meanwhile (the
    /// first window read its settings 9 s late).
    pub fn prune_history(&self, days: u32) -> Result<usize> {
        const STEP: i64 = 500;
        let cutoff = now_ms() - i64::from(days) * 24 * 60 * 60 * 1000;
        let mut deleted = 0;
        loop {
            let n = self.with(|db| {
                Ok(db.execute(
                    "DELETE FROM history WHERE id IN (
                         SELECT id FROM history WHERE updated_at < ?1
                         AND NOT (closed = 0 AND id = (SELECT MAX(id) FROM history h WHERE h.tab_id = history.tab_id))
                         LIMIT ?2)",
                    params![cutoff, STEP],
                )?)
            })?;
            deleted += n;
            if (n as i64) < STEP {
                return Ok(deleted);
            }
            // let calls that wait for the store go first (the lock is not fair:
            // giving up the time slice alone lets this thread take it again)
            std::thread::sleep(std::time::Duration::from_millis(5));
        }
    }

    // -- catalog cache ----------------------------------------------------

    pub fn save_catalog(&self, connection_id: &str, database: &str, snap: &CatalogSnapshot) -> Result<()> {
        let json = serde_json::to_string(snap)?;
        self.with(|db| {
            db.execute(
                "INSERT INTO catalog_cache (connection_id, database, json, loaded_at) VALUES (?1, ?2, ?3, ?4)
                 ON CONFLICT(connection_id, database) DO UPDATE SET json = excluded.json, loaded_at = excluded.loaded_at",
                params![connection_id, database, json, now_ms()],
            )?;
            Ok(())
        })
    }

    pub fn load_catalog(&self, connection_id: &str, database: &str) -> Result<Option<(CatalogSnapshot, i64)>> {
        self.with(|db| {
            let row: Option<(String, i64)> = db
                .query_row(
                    "SELECT json, loaded_at FROM catalog_cache WHERE connection_id = ?1 AND database = ?2",
                    params![connection_id, database],
                    |r| Ok((r.get(0)?, r.get(1)?)),
                )
                .optional()?;
            Ok(match row {
                Some((json, at)) => Some((serde_json::from_str(&json)?, at)),
                None => None,
            })
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn entry(tab: &str, text: &str, kind: HistoryKind) -> NewHistory {
        NewHistory {
            tab_id: tab.into(),
            title: "Query 1".into(),
            connection_name: Some("local".into()),
            database: Some("Shop".into()),
            dialect: "mssql".into(),
            text: text.into(),
            kind,
        }
    }

    #[test]
    fn rows_a_newer_version_wrote_do_not_hide_the_others() {
        let s = Store::open_in_memory().unwrap();
        s.save_connection(&ConnectionConfig {
            id: "c1".into(),
            name: "Local".into(),
            ..Default::default()
        })
        .unwrap();
        s.with(|db| {
            db.execute(
                "INSERT INTO connections (id, json, sort) VALUES ('c2', '{\"id\":\"c2\",\"dialect\":\"oracle\"}', 9)",
                [],
            )?;
            Ok(())
        })
        .unwrap();
        let list = s.list_connections().unwrap();
        assert_eq!(list.iter().map(|c| c.id.as_str()).collect::<Vec<_>>(), vec!["c1"]);
        // the unreadable one is kept as it is
        assert!(s.get_connection("c2").is_err());
    }

    #[test]
    fn connections_roundtrip() {
        let s = Store::open_in_memory().unwrap();
        let cfg = ConnectionConfig {
            id: "c1".into(),
            name: "Local".into(),
            host: "localhost".into(),
            ..Default::default()
        };
        s.save_connection(&cfg).unwrap();
        s.save_connection(&ConnectionConfig {
            name: "Renamed".into(),
            ..cfg.clone()
        })
        .unwrap();
        let all = s.list_connections().unwrap();
        assert_eq!(all.len(), 1);
        assert_eq!(all[0].name, "Renamed");
    }

    #[test]
    fn settings_roundtrip() {
        let s = Store::open_in_memory().unwrap();
        s.set_setting("format", &sqlotus_core::FormatOptions::default())
            .unwrap();
        let back: sqlotus_core::FormatOptions = s.get_setting("format").unwrap().unwrap();
        assert_eq!(back, sqlotus_core::FormatOptions::default());
        assert!(s.get_setting::<String>("missing").unwrap().is_none());
    }

    #[test]
    fn history_versions_and_search() {
        let s = Store::open_in_memory().unwrap();
        let a = s
            .record_history(&entry("t1", "SELECT * FROM Orders", HistoryKind::Autosave))
            .unwrap();
        // autosave right after: same entry updated
        let b = s
            .record_history(&entry("t1", "SELECT * FROM Orders WHERE Id = 1", HistoryKind::Autosave))
            .unwrap();
        assert_eq!(a, b);
        // execute: new version
        let c = s
            .record_history(&entry("t1", "SELECT * FROM Orders WHERE Id = 2", HistoryKind::Execute))
            .unwrap();
        assert_ne!(b, c);
        // same text again: no new version
        let d = s
            .record_history(&entry("t1", "SELECT * FROM Orders WHERE Id = 2", HistoryKind::Autosave))
            .unwrap();
        assert_eq!(c, d);
        s.record_history(&entry("t2", "UPDATE Customers SET Name = 'x'", HistoryKind::Execute))
            .unwrap();

        assert_eq!(s.tab_history("t1").unwrap().len(), 2);
        let hits = s.search_history("orders id", 10).unwrap();
        assert_eq!(hits.len(), 2);
        let hits = s.search_history("Customers", 10).unwrap();
        assert_eq!(hits.len(), 1);
        let hits = s.search_history("Id = 2", 10).unwrap(); // short term -> LIKE path
        assert_eq!(hits.len(), 1);
        assert_eq!(s.search_history("", 10).unwrap().len(), 3);

        s.mark_tab_closed("t2").unwrap();
        let closed = s.recently_closed(5).unwrap();
        assert_eq!(closed.len(), 1);
        assert!(closed[0].text.starts_with("UPDATE"));
    }

    #[test]
    fn old_history_is_pruned_but_open_tabs_keep_their_latest_version() {
        let s = Store::open_in_memory().unwrap();
        let old1 = s
            .record_history(&entry("open", "SELECT 1 AS first_version", HistoryKind::Execute))
            .unwrap();
        let old2 = s
            .record_history(&entry("open", "SELECT 2 AS latest_version", HistoryKind::Execute))
            .unwrap();
        let gone = s
            .record_history(&entry("closed", "SELECT 3 AS closed_tab", HistoryKind::Execute))
            .unwrap();
        s.mark_tab_closed("closed").unwrap();
        let recent = s
            .record_history(&entry("recent", "SELECT 4 AS recent_one", HistoryKind::Execute))
            .unwrap();
        let long_ago = now_ms() - 40 * 24 * 60 * 60 * 1000;
        s.with(|db| {
            db.execute(
                "UPDATE history SET updated_at = ?1 WHERE id IN (?2, ?3, ?4)",
                params![long_ago, old1, old2, gone],
            )?;
            Ok(())
        })
        .unwrap();
        assert_eq!(s.prune_history(30).unwrap(), 2);
        let left: Vec<i64> = s.search_history("", 10).unwrap().iter().map(|h| h.id).collect();
        assert_eq!(left, vec![recent, old2]);
        // the text index forgets them too
        assert!(s.search_history("closed_tab", 10).unwrap().is_empty());
        assert!(s.search_history("first_version", 10).unwrap().is_empty());
        assert_eq!(s.prune_history(30).unwrap(), 0);
    }

    /// A backlog larger than one step of the pruning goes in several, and
    /// the open tab still keeps its latest version.
    #[test]
    fn a_large_history_is_pruned_in_steps() {
        let s = Store::open_in_memory().unwrap();
        let long_ago = now_ms() - 40 * 24 * 60 * 60 * 1000;
        s.with(|db| {
            for i in 0..1234 {
                db.execute(
                    "INSERT INTO history (tab_id, title, dialect, text, kind, created_at, updated_at) \
                     VALUES (?1, 'q', 'mssql', ?2, 'execute', ?3, ?3)",
                    params![
                        if i % 2 == 0 { "open" } else { "other" },
                        format!("SELECT {i}"),
                        long_ago
                    ],
                )?;
            }
            Ok(())
        })
        .unwrap();
        // the latest version of each tab, both open, stays
        assert_eq!(s.prune_history(30).unwrap(), 1232);
        let left: Vec<String> = s.search_history("", 10).unwrap().into_iter().map(|h| h.text).collect();
        assert_eq!(left, vec!["SELECT 1233", "SELECT 1232"]);
    }

    #[test]
    fn imports_snippet_files() {
        let dir = std::env::temp_dir().join(format!("sqlotus-snip-{}", now_ms()));
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(
            dir.join("ssf-1.json"),
            r#"{"id":"abc","prefix":"ssf","description":"Select star","body":"SELECT * FROM $CURSOR$","placeholders":[]}"#,
        )
        .unwrap();
        std::fs::write(dir.join("broken.json"), "{").unwrap();
        let s = Store::open_in_memory().unwrap();
        assert_eq!(s.import_snippets(&dir).unwrap(), 1);
        assert_eq!(s.list_snippets().unwrap()[0].prefix, "ssf");
        std::fs::remove_dir_all(dir).ok();
    }
}
