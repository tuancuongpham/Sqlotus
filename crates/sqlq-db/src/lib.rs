//! Database access: connecting, running batches with text results, and
//! loading the metadata catalog used by completion and search.

mod metadata;
mod mssql;
mod mysql;
mod pg;

use std::time::{Duration, Instant};

use serde::{Deserialize, Serialize};
use sqlq_core::Dialect;
use sqlq_core::catalog::ObjectKind;

pub use metadata::{CatalogSnapshot, load_catalog};
pub use sqlq_core::catalog::ObjectDefinition;

#[derive(Debug, thiserror::Error)]
pub enum DbError {
    #[error("connection failed: {0}")]
    Connect(String),
    #[error("{0}")]
    Query(String),
    #[error("timed out after {0:?}")]
    Timeout(Duration),
    #[error("{0}")]
    Unsupported(String),
}

pub type Result<T, E = DbError> = std::result::Result<T, E>;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum AuthKind {
    #[default]
    Sql,
    /// Windows integrated authentication (SQL Server only).
    Windows,
}

#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(default, rename_all = "camelCase")]
pub struct ConnectionConfig {
    pub id: String,
    pub name: String,
    pub dialect: Dialect,
    /// Host name. SQL Server also accepts `host\instance` and `host,port`.
    pub host: String,
    pub port: Option<u16>,
    pub database: Option<String>,
    pub auth: AuthKind,
    pub user: Option<String>,
    /// Encrypt the whole session (otherwise SQL Server only encrypts login).
    pub encrypt: bool,
    pub trust_server_certificate: bool,
    /// Tab colour, e.g. red for production.
    pub color: Option<String>,
}

impl ConnectionConfig {
    pub fn default_port(&self) -> u16 {
        match self.dialect {
            Dialect::MsSql => 1433,
            Dialect::Postgres => 5432,
            Dialect::MySql => 3306,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ResultColumn {
    pub name: String,
    pub type_name: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ResultSet {
    pub columns: Vec<ResultColumn>,
    pub rows: Vec<Vec<Option<String>>>,
    /// More rows existed than `Limits::max_rows`.
    pub truncated: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum MessageKind {
    Info,
    Error,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct Message {
    pub kind: MessageKind,
    pub text: String,
}

/// Output of one batch.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct BatchOutput {
    pub result_sets: Vec<ResultSet>,
    pub messages: Vec<Message>,
    pub elapsed_ms: u64,
    pub failed: bool,
}

impl BatchOutput {
    fn info(&mut self, text: impl Into<String>) {
        self.messages.push(Message {
            kind: MessageKind::Info,
            text: text.into(),
        });
    }

    fn error(&mut self, text: impl Into<String>) {
        self.failed = true;
        self.messages.push(Message {
            kind: MessageKind::Error,
            text: text.into(),
        });
    }
}

#[derive(Debug, Clone, Copy)]
pub struct Limits {
    pub max_rows: usize,
    /// Longer cell values are cut (like SSMS's grid limit).
    pub max_cell_chars: usize,
}

impl Default for Limits {
    fn default() -> Self {
        Self {
            max_rows: 100_000,
            max_cell_chars: 65_535,
        }
    }
}

pub(crate) fn clip(mut s: String, max: usize) -> String {
    if let Some((i, _)) = s.char_indices().nth(max) {
        s.truncate(i);
        s.push('…');
    }
    s
}

pub enum Connection {
    MsSql(Box<mssql::MssqlConn>),
    Postgres(pg::PgConn),
    MySql(mysql::MySqlConn),
}

/// Something that can interrupt a running query from another task.
#[derive(Clone)]
pub enum CancelHandle {
    Postgres(pg::PgCancel),
    MySql(mysql::MySqlCancel),
    /// SQL Server: the running task is aborted and the connection dropped.
    None,
}

impl CancelHandle {
    pub async fn cancel(&self) -> Result<()> {
        match self {
            CancelHandle::Postgres(c) => c.cancel().await,
            CancelHandle::MySql(c) => c.cancel().await,
            CancelHandle::None => Ok(()),
        }
    }
}

const CONNECT_TIMEOUT: Duration = Duration::from_secs(20);

impl Connection {
    pub async fn open(cfg: &ConnectionConfig, password: Option<&str>) -> Result<Self> {
        let fut = async {
            Ok(match cfg.dialect {
                Dialect::MsSql => {
                    Connection::MsSql(Box::new(mssql::MssqlConn::open(cfg, password).await?))
                }
                Dialect::Postgres => Connection::Postgres(pg::PgConn::open(cfg, password).await?),
                Dialect::MySql => Connection::MySql(mysql::MySqlConn::open(cfg, password).await?),
            })
        };
        tokio::time::timeout(CONNECT_TIMEOUT, fut)
            .await
            .map_err(|_| DbError::Timeout(CONNECT_TIMEOUT))?
    }

    pub fn dialect(&self) -> Dialect {
        match self {
            Connection::MsSql(_) => Dialect::MsSql,
            Connection::Postgres(_) => Dialect::Postgres,
            Connection::MySql(_) => Dialect::MySql,
        }
    }

    pub fn cancel_handle(&self) -> CancelHandle {
        match self {
            Connection::MsSql(_) => CancelHandle::None,
            Connection::Postgres(c) => CancelHandle::Postgres(c.cancel_handle()),
            Connection::MySql(c) => CancelHandle::MySql(c.cancel_handle()),
        }
    }

    /// Run one batch (no `GO` handling here; see [`execute_script`]).
    pub async fn execute(&mut self, sql: &str, limits: &Limits) -> BatchOutput {
        let start = Instant::now();
        let mut out = match self {
            Connection::MsSql(c) => c.execute(sql, limits).await,
            Connection::Postgres(c) => c.execute(sql, limits).await,
            Connection::MySql(c) => c.execute(sql, limits).await,
        };
        out.elapsed_ms = start.elapsed().as_millis() as u64;
        out
    }

    /// Rows of the first result set as strings; errors become `DbError`.
    pub async fn query_strings(&mut self, sql: &str) -> Result<Vec<Vec<Option<String>>>> {
        let limits = Limits {
            max_rows: usize::MAX,
            max_cell_chars: usize::MAX,
        };
        let out = self.execute(sql, &limits).await;
        if out.failed {
            let msg = out
                .messages
                .iter()
                .filter(|m| m.kind == MessageKind::Error)
                .map(|m| m.text.as_str())
                .collect::<Vec<_>>();
            return Err(DbError::Query(msg.join("\n")));
        }
        Ok(out
            .result_sets
            .into_iter()
            .next()
            .map(|r| r.rows)
            .unwrap_or_default())
    }

    pub async fn current_database(&mut self) -> Result<String> {
        let sql = match self.dialect() {
            Dialect::MsSql => "SELECT DB_NAME()",
            Dialect::Postgres => "SELECT current_database()",
            Dialect::MySql => "SELECT DATABASE()",
        };
        let rows = self.query_strings(sql).await?;
        Ok(rows
            .into_iter()
            .next()
            .and_then(|r| r.into_iter().next().flatten())
            .unwrap_or_default())
    }

    pub async fn list_databases(&mut self) -> Result<Vec<String>> {
        let sql = match self.dialect() {
            Dialect::MsSql => {
                "SELECT name FROM sys.databases WHERE state = 0 AND HAS_DBACCESS(name) = 1 ORDER BY name"
            }
            Dialect::Postgres => {
                "SELECT datname FROM pg_database WHERE NOT datistemplate AND datallowconn ORDER BY 1"
            }
            Dialect::MySql => "SELECT SCHEMA_NAME FROM information_schema.SCHEMATA ORDER BY 1",
        };
        let rows = self.query_strings(sql).await?;
        Ok(rows
            .into_iter()
            .filter_map(|r| r.into_iter().next().flatten())
            .collect())
    }

    /// Switch database on this connection. Postgres cannot switch; callers
    /// reconnect with a different `database` instead.
    pub async fn use_database(&mut self, db: &str) -> Result<()> {
        let sql = match self.dialect() {
            Dialect::MsSql => format!("USE [{}]", db.replace(']', "]]")),
            Dialect::MySql => format!("USE `{}`", db.replace('`', "``")),
            Dialect::Postgres => {
                return Err(DbError::Unsupported(
                    "Postgres needs a new connection to change database".into(),
                ));
            }
        };
        self.query_strings(&sql).await.map(|_| ())
    }

    pub async fn close(self) {
        if let Connection::MsSql(c) = self {
            c.close().await;
        }
    }
}

/// Output of one batch of a script, with its position in the source.
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ScriptBatch {
    pub start: usize,
    pub end: usize,
    pub output: BatchOutput,
}

/// Run a whole script: T-SQL is split on `GO` (honouring `GO n`); other
/// dialects run as a single multi-statement batch. Like SSMS, later batches
/// still run after a failed one.
pub async fn execute_script(
    conn: &mut Connection,
    script: &str,
    limits: &Limits,
) -> Vec<ScriptBatch> {
    let mut results = Vec::new();
    for batch in sqlq_core::statements::split_batches(script, conn.dialect()) {
        let text = batch.span.text(script);
        for _ in 0..batch.repeat {
            let output = conn.execute(text, limits).await;
            results.push(ScriptBatch {
                start: batch.span.start,
                end: batch.span.end,
                output,
            });
        }
    }
    results
}

pub(crate) fn kind_from_mssql(code: &str) -> Option<ObjectKind> {
    Some(match code.trim() {
        "U" => ObjectKind::Table,
        "V" => ObjectKind::View,
        "P" | "PC" => ObjectKind::Procedure,
        "FN" | "FS" => ObjectKind::ScalarFunction,
        "IF" | "TF" | "FT" => ObjectKind::TableFunction,
        "TR" => ObjectKind::Trigger,
        "SN" => ObjectKind::Synonym,
        "SO" => ObjectKind::Sequence,
        _ => return None,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn clip_long_values() {
        assert_eq!(clip("abcdef".into(), 3), "abc…");
        assert_eq!(clip("ab".into(), 3), "ab");
    }
}
