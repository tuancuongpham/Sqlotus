//! Database access: connecting, running batches with text results, and
//! loading the metadata catalog used by completion and search.

mod endpoint;
mod metadata;
mod mssql;
mod mysql;
mod pg;
mod schema;

use std::time::{Duration, Instant};

use serde::{Deserialize, Serialize};
use sqlotus_core::Dialect;
use sqlotus_core::catalog::ObjectKind;
use sqlotus_core::plan::StatementPlan;
use sqlotus_core::variables::VariableValue;

pub use endpoint::Endpoint;
pub use metadata::{CatalogSnapshot, load_catalog};
pub use schema::load_schema;
pub use sqlotus_core::catalog::ObjectDefinition;

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
    /// Host name as typed: SQL Server also accepts `host\instance` and
    /// `host,port`, PostgreSQL and MySQL `host:port` (see [`Endpoint`]).
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
    /// Refuse statements that change data or schema. PostgreSQL and MySQL
    /// sessions are also read-only on the server; SQL Server has no such mode.
    pub read_only: bool,
    /// Start a transaction before changes and leave it open until the user
    /// commits or rolls back.
    pub manual_commit: bool,
    /// `dev`, `test`, `staging` or `prod`: groups connections and sets
    /// safety defaults (production asks before every change).
    pub environment: Option<String>,
    /// A folder of the user's in the connection list (e.g. a customer).
    pub group: Option<String>,
}

impl ConnectionConfig {
    /// Where to connect: the host field read for this server, with the Port
    /// field winning over a port typed in the host.
    pub fn endpoint(&self) -> Endpoint {
        let mut e = Endpoint::parse(self.dialect, &self.host);
        e.port = self.port.or(e.port);
        e
    }

    /// The server's standard port, used when none is given.
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
    /// Values cut to `Limits::max_cell_chars` for display: editing or
    /// exporting them would use the cut text.
    pub clipped: usize,
}

impl ResultSet {
    /// Add a row, cutting values longer than `max` characters.
    pub(crate) fn push_cells(&mut self, cells: impl IntoIterator<Item = Option<String>>, max: usize) {
        let mut cut = 0;
        let row = cells
            .into_iter()
            .map(|c| {
                c.map(|s| {
                    let (s, was_cut) = clip(s, max);
                    cut += usize::from(was_cut);
                    s
                })
            })
            .collect();
        self.clipped += cut;
        self.rows.push(row);
    }
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
    /// Rows changed by INSERT/UPDATE/DELETE/MERGE (0 when not reported:
    /// SQL Server only reports it for counted runs).
    pub rows_affected: u64,
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

    /// The error messages, one per line.
    pub fn errors(&self) -> String {
        self.messages
            .iter()
            .filter(|m| m.kind == MessageKind::Error)
            .map(|m| m.text.as_str())
            .collect::<Vec<_>>()
            .join("\n")
    }
}

#[derive(Debug, Clone, Copy)]
pub struct Limits {
    pub max_rows: usize,
    /// Longer cell values are cut to keep the grid responsive.
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

/// `s` cut to `max` characters (with `…`), and whether it was cut.
pub(crate) fn clip(mut s: String, max: usize) -> (String, bool) {
    if let Some((i, _)) = s.char_indices().nth(max) {
        s.truncate(i);
        s.push('…');
        return (s, true);
    }
    (s, false)
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
                Dialect::MsSql => Connection::MsSql(Box::new(mssql::MssqlConn::open(cfg, password).await?)),
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

    /// Like [`execute`](Self::execute), but SQL Server also reports the rows
    /// each statement changed (see `MssqlConn::execute_counted`).
    pub async fn execute_counted(&mut self, sql: &str, limits: &Limits) -> BatchOutput {
        match self {
            Connection::MsSql(c) => {
                let start = Instant::now();
                let mut out = c.execute_counted(sql, limits).await;
                out.elapsed_ms = start.elapsed().as_millis() as u64;
                out
            }
            _ => self.execute(sql, limits).await,
        }
    }

    /// Run one INSERT, UPDATE or DELETE and return how many rows it changed
    /// itself: saving an edited result row needs exactly one. SQL Server's
    /// counts take in the rows its triggers change (an AFTER UPDATE trigger
    /// that matched nothing can still report a row), so there the
    /// statement's own `@@ROWCOUNT` is read.
    pub async fn rows_changed(&mut self, sql: &str, limits: &Limits) -> std::result::Result<u64, String> {
        if self.dialect() != Dialect::MsSql {
            let out = self.execute(sql, limits).await;
            return if out.failed {
                Err(out.errors())
            } else {
                Ok(out.rows_affected)
            };
        }
        let checked = format!("{};\nSELECT @@ROWCOUNT;", sql.trim_end().trim_end_matches(';'));
        let out = self.execute(&checked, limits).await;
        if out.failed {
            return Err(out.errors());
        }
        out.result_sets
            .last()
            .and_then(|rs| rs.rows.first())
            .and_then(|row| row.first().cloned().flatten())
            .and_then(|n| n.parse().ok())
            .ok_or_else(|| "the server did not say how many rows changed".to_string())
    }

    /// The statement that starts a transaction in this dialect.
    pub fn begin_statement(&self) -> &'static str {
        match self.dialect() {
            Dialect::MsSql => "BEGIN TRANSACTION",
            Dialect::Postgres => "BEGIN",
            Dialect::MySql => "START TRANSACTION",
        }
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
        Ok(out.result_sets.into_iter().next().map(|r| r.rows).unwrap_or_default())
    }

    /// Number of transactions left open on this connection (0 when none).
    pub async fn open_transactions(&mut self) -> Result<u32> {
        let sql = match self.dialect() {
            Dialect::MsSql => "SELECT @@TRANCOUNT",
            // the check itself runs in its own transaction unless one is open
            Dialect::Postgres => {
                "SELECT CASE WHEN xact_start <> query_start THEN 1 ELSE 0 END FROM pg_stat_activity WHERE pid = pg_backend_pid()"
            }
            Dialect::MySql => {
                let Connection::MySql(c) = self else { unreachable!() };
                return Ok(u32::from(c.in_transaction().await?));
            }
        };
        let rows = match self.query_strings(sql).await {
            Ok(rows) => rows,
            // Postgres refuses every query in a failed transaction until it ends
            Err(e)
                if self.dialect() == Dialect::Postgres && e.to_string().contains("current transaction is aborted") =>
            {
                return Ok(1);
            }
            Err(e) => return Err(e),
        };
        Ok(rows
            .first()
            .and_then(|r| r.first().cloned().flatten())
            .and_then(|v| v.parse().ok())
            .unwrap_or(0))
    }

    /// Whether the open transaction has written anything: `None` when the
    /// server does not tell (no permission to look, no transaction, a failed
    /// transaction).
    pub async fn transaction_wrote(&mut self) -> Option<bool> {
        let sql = match self.dialect() {
            // log records this session's transaction wrote outside tempdb
            // (reading them needs VIEW SERVER STATE)
            Dialect::MsSql => {
                "SELECT CASE WHEN EXISTS (SELECT 1 FROM sys.dm_tran_session_transactions st \
                 JOIN sys.dm_tran_database_transactions dt ON dt.transaction_id = st.transaction_id \
                 WHERE st.session_id = @@SPID AND dt.database_id <> 2 AND dt.database_transaction_log_record_count > 0) \
                 THEN 1 ELSE 0 END"
            }
            // a transaction gets an id when it first writes
            Dialect::Postgres => "SELECT CASE WHEN txid_current_if_assigned() IS NULL THEN 0 ELSE 1 END",
            // rows InnoDB changed in this connection's transaction (needs
            // PROCESS); the table is a copy refreshed at most every 0.1 s, so
            // wait for a copy made after the changes
            Dialect::MySql => {
                self.query_strings("DO SLEEP(0.11)").await.ok()?;
                "SELECT CASE WHEN COALESCE(SUM(trx_rows_modified), 0) > 0 THEN 1 ELSE 0 END \
                 FROM information_schema.innodb_trx WHERE trx_mysql_thread_id = CONNECTION_ID()"
            }
        };
        let rows = self.query_strings(sql).await.ok()?;
        match rows.first()?.first()?.as_deref()? {
            "1" => Some(true),
            "0" => Some(false),
            _ => None,
        }
    }

    /// Estimated execution plans of the statements in `script` (nothing is
    /// executed).
    pub async fn explain(&mut self, script: &str) -> ExplainOutput {
        let limits = Limits {
            max_rows: 10_000,
            max_cell_chars: usize::MAX,
        };
        let mut out = ExplainOutput::default();
        let first_text = |o: &BatchOutput| -> Vec<String> {
            o.result_sets
                .iter()
                .filter_map(|rs| rs.rows.first().and_then(|r| r.first().cloned().flatten()))
                .collect()
        };
        let errors = |o: &BatchOutput| -> Vec<String> {
            o.messages
                .iter()
                .filter(|m| m.kind == MessageKind::Error)
                .map(|m| m.text.clone())
                .collect()
        };
        let dialect = self.dialect();
        if dialect == Dialect::MsSql {
            let on = self.execute("SET SHOWPLAN_XML ON", &limits).await;
            if on.failed {
                out.errors.extend(errors(&on));
                return out;
            }
            let batches = match script_batches(script, dialect) {
                Ok((b, _)) => b,
                Err(e) => {
                    out.errors.push(e);
                    let _ = self.execute("SET SHOWPLAN_XML OFF", &limits).await;
                    return out;
                }
            };
            for batch in batches {
                let o = self.execute(&batch.text, &limits).await;
                for xml in first_text(&o) {
                    match sqlotus_core::plan::parse_showplan_xml(&xml) {
                        Ok(p) => out.plans.extend(p),
                        Err(e) => out.errors.push(e),
                    }
                }
                out.errors.extend(errors(&o));
            }
            let _ = self.execute("SET SHOWPLAN_XML OFF", &limits).await;
            return out;
        }
        for span in sqlotus_core::statements::split_statements(script, dialect) {
            let stmt = span.text(script).trim().trim_end_matches(';').trim();
            let first = stmt.split_whitespace().next().unwrap_or("").to_ascii_uppercase();
            if !matches!(
                first.as_str(),
                "SELECT" | "INSERT" | "UPDATE" | "DELETE" | "WITH" | "VALUES" | "MERGE" | "TABLE" | "REPLACE"
            ) {
                continue;
            }
            if dialect == Dialect::Postgres {
                let o = self.execute(&format!("EXPLAIN (FORMAT JSON) {stmt}"), &limits).await;
                match first_text(&o).first() {
                    Some(json) => match sqlotus_core::plan::parse_pg_json(stmt, json) {
                        Ok(p) => out.plans.push(p),
                        Err(e) => out.errors.push(e),
                    },
                    None => out.errors.extend(errors(&o)),
                }
            } else {
                let o = self.execute(&format!("EXPLAIN FORMAT=TREE {stmt}"), &limits).await;
                if let (false, Some(text)) = (o.failed, first_text(&o).first()) {
                    out.plans.push(sqlotus_core::plan::parse_mysql_tree(stmt, text));
                    continue;
                }
                // MariaDB and older MySQL: the classic table
                let o = self.execute(&format!("EXPLAIN {stmt}"), &limits).await;
                match o.result_sets.first() {
                    Some(rs) if !o.failed => {
                        let cols: Vec<String> = rs.columns.iter().map(|c| c.name.clone()).collect();
                        out.plans
                            .push(sqlotus_core::plan::parse_mysql_table(stmt, &cols, &rs.rows));
                    }
                    _ => out.errors.extend(errors(&o)),
                }
            }
        }
        out
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
            Dialect::MsSql => "SELECT name FROM sys.databases WHERE state = 0 AND HAS_DBACCESS(name) = 1 ORDER BY name",
            Dialect::Postgres => "SELECT datname FROM pg_database WHERE NOT datistemplate AND datallowconn ORDER BY 1",
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

/// Estimated plans, and errors for statements that could not be explained.
#[derive(Debug, Clone, Default, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ExplainOutput {
    pub plans: Vec<StatementPlan>,
    pub errors: Vec<String>,
}

/// How [`execute_script_with`] runs a script.
#[derive(Debug, Clone, Default)]
pub struct ScriptOptions {
    /// Values for variables the script does not declare.
    pub variables: Vec<VariableValue>,
    /// Report the rows each statement changes (see [`Connection::execute_counted`]).
    pub counted: bool,
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
/// dialects run as a single multi-statement batch. Later batches still run
/// after a failed one, unless a T-SQL script says `:on error exit`.
pub async fn execute_script(conn: &mut Connection, script: &str, limits: &Limits) -> Vec<ScriptBatch> {
    execute_script_with(conn, script, limits, &ScriptOptions::default()).await
}

/// One batch of a script as the server receives it.
struct RunBatch {
    /// Byte offsets in the script.
    start: usize,
    end: usize,
    text: String,
    repeat: u32,
}

/// The batches of a script and whether to stop at the first failed one.
/// T-SQL SQLCMD command lines (`:setvar`, `:on error`) are taken out and
/// `$(variables)` replaced; a command Sqlotus cannot run is an error.
fn script_batches(script: &str, dialect: Dialect) -> std::result::Result<(Vec<RunBatch>, bool), String> {
    let sqlcmd = match dialect {
        Dialect::MsSql => sqlotus_core::sqlcmd::Sqlcmd::parse(script)?,
        _ => None,
    };
    let source = sqlcmd.as_ref().map_or(script, |c| c.text.as_str());
    let batches = sqlotus_core::statements::split_batches(source, dialect)
        .into_iter()
        .map(|b| RunBatch {
            start: b.span.start,
            end: b.span.end,
            text: match &sqlcmd {
                Some(c) => c.expand(b.span.start..b.span.end),
                None => b.span.text(script).to_string(),
            },
            repeat: b.repeat,
        })
        .collect();
    Ok((batches, sqlcmd.is_some_and(|c| c.stop_on_error)))
}

/// [`execute_script`] with variable values and/or row counting.
pub async fn execute_script_with(
    conn: &mut Connection,
    script: &str,
    limits: &Limits,
    options: &ScriptOptions,
) -> Vec<ScriptBatch> {
    let dialect = conn.dialect();
    let (batches, stop_on_error) = match script_batches(script, dialect) {
        Ok(b) => b,
        Err(e) => {
            // nothing runs when a SQLCMD line cannot be honoured
            let mut output = BatchOutput::default();
            output.error(e);
            return vec![ScriptBatch {
                start: 0,
                end: script.len(),
                output,
            }];
        }
    };
    let mut results = Vec::new();
    for batch in batches {
        let prelude = sqlotus_core::variables::prelude(&batch.text, dialect, &options.variables);
        let text = format!("{prelude}{}", batch.text);
        for _ in 0..batch.repeat {
            let output = if options.counted {
                conn.execute_counted(&text, limits).await
            } else {
                conn.execute(&text, limits).await
            };
            let failed = output.failed;
            results.push(ScriptBatch {
                start: batch.start,
                end: batch.end,
                output,
            });
            if failed && stop_on_error {
                return results;
            }
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
        assert_eq!(clip("abcdef".into(), 3), ("abc…".to_string(), true));
        assert_eq!(clip("abc".into(), 3), ("abc".to_string(), false));
        assert_eq!(clip("ẫẫẫẫ".into(), 2), ("ẫẫ…".to_string(), true));
        let mut rs = ResultSet {
            columns: vec![],
            rows: vec![],
            truncated: false,
            clipped: 0,
        };
        rs.push_cells([Some("abcdef".to_string()), None, Some("ab".to_string())], 3);
        rs.push_cells([Some("xyzxyz".to_string())], 3);
        assert_eq!(rs.clipped, 2);
        assert_eq!(rs.rows[0], vec![Some("abc…".to_string()), None, Some("ab".to_string())]);
    }
}
