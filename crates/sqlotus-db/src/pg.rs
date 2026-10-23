//! PostgreSQL via tokio-postgres, using the simple (text) protocol so every
//! value arrives as text and multi-statement scripts work.

use std::sync::{Arc, Mutex};

use futures_util::StreamExt;
use postgres_native_tls::MakeTlsConnector;
use tokio_postgres::config::SslMode;
use tokio_postgres::{AsyncMessage, CancelToken, Client, SimpleQueryMessage};

use crate::{BatchOutput, ConnectionConfig, DbError, Limits, Result, ResultColumn, ResultSet};

pub struct PgConn {
    client: Client,
    notices: Arc<Mutex<Vec<String>>>,
    tls: MakeTlsConnector,
}

#[derive(Clone)]
pub struct PgCancel {
    token: CancelToken,
    tls: MakeTlsConnector,
}

impl PgCancel {
    pub async fn cancel(&self) -> Result<()> {
        self.token
            .cancel_query(self.tls.clone())
            .await
            .map_err(|e| DbError::Query(e.to_string()))
    }
}

fn tls(trust: bool) -> Result<MakeTlsConnector> {
    let connector = native_tls::TlsConnector::builder()
        .danger_accept_invalid_certs(trust)
        .danger_accept_invalid_hostnames(trust)
        .build()
        .map_err(|e| DbError::Connect(e.to_string()))?;
    Ok(MakeTlsConnector::new(connector))
}

impl PgConn {
    pub async fn open(cfg: &ConnectionConfig, password: Option<&str>) -> Result<Self> {
        let endpoint = cfg.endpoint();
        let mut config = tokio_postgres::Config::new();
        config
            .host(&endpoint.host)
            .port(endpoint.port.unwrap_or(cfg.default_port()))
            .user(cfg.user.as_deref().unwrap_or("postgres"))
            .dbname(cfg.database.as_deref().filter(|d| !d.is_empty()).unwrap_or("postgres"))
            .application_name("Sqlotus")
            .ssl_mode(if cfg.encrypt { SslMode::Require } else { SslMode::Prefer });
        if let Some(p) = password {
            config.password(p);
        }
        if cfg.read_only {
            // the server itself refuses writes, also inside functions
            config.options("-c default_transaction_read_only=on");
        }
        let tls = tls(cfg.trust_server_certificate || !cfg.encrypt)?;
        let (client, mut connection) = config
            .connect(tls.clone())
            .await
            .map_err(|e| DbError::Connect(pg_error(&e)))?;

        // Drive the connection and collect NOTICE / RAISE messages.
        let notices = Arc::new(Mutex::new(Vec::new()));
        let sink = notices.clone();
        tokio::spawn(async move {
            let mut messages = futures_util::stream::poll_fn(move |cx| connection.poll_message(cx));
            while let Some(msg) = messages.next().await {
                match msg {
                    Ok(AsyncMessage::Notice(n)) => {
                        if let Ok(mut v) = sink.lock() {
                            v.push(format!("{}: {}", n.severity(), n.message()));
                        }
                    }
                    Ok(_) => {}
                    Err(_) => break,
                }
            }
        });
        Ok(Self { client, notices, tls })
    }

    pub fn cancel_handle(&self) -> PgCancel {
        PgCancel {
            token: self.client.cancel_token(),
            tls: self.tls.clone(),
        }
    }

    pub async fn execute(&mut self, sql: &str, limits: &Limits) -> BatchOutput {
        let mut out = BatchOutput::default();
        if let Some(refused) = client_copy(sql) {
            out.error(refused);
            return out;
        }
        let result = self.run(sql, limits).await;
        let notices: Vec<String> = self
            .notices
            .lock()
            .map(|mut v| std::mem::take(&mut *v))
            .unwrap_or_default();
        for n in notices {
            out.info(n);
        }
        match result {
            Ok(ran) => {
                out.messages.extend(ran.messages);
                out.result_sets = ran.result_sets;
                out.rows_affected = ran.rows_affected;
            }
            Err(e) => {
                out.error(pg_error(&e));
                return out;
            }
        }
        self.add_types(sql, &mut out).await;
        out
    }

    /// Run the statements, reading rows as they arrive: the ones past the
    /// limits are dropped there, so a huge result never sits in memory.
    async fn run(&self, sql: &str, limits: &Limits) -> std::result::Result<BatchOutput, tokio_postgres::Error> {
        let mut out = BatchOutput::default();
        let messages = self.client.simple_query_raw(sql).await?;
        let mut messages = std::pin::pin!(messages);
        let mut current: Option<ResultSet> = None;
        let mut kept = 0;
        while let Some(msg) = messages.next().await {
            match msg? {
                SimpleQueryMessage::RowDescription(cols) => {
                    let columns = cols
                        .iter()
                        .map(|c| ResultColumn {
                            name: c.name().to_string(),
                            type_name: String::new(),
                        })
                        .collect();
                    current = Some(ResultSet {
                        columns,
                        rows: Vec::new(),
                        truncated: false,
                        clipped: 0,
                    });
                }
                SimpleQueryMessage::Row(row) => {
                    let rs = current.get_or_insert_with(|| ResultSet {
                        columns: row
                            .columns()
                            .iter()
                            .map(|c| ResultColumn {
                                name: c.name().to_string(),
                                type_name: String::new(),
                            })
                            .collect(),
                        rows: Vec::new(),
                        truncated: false,
                        clipped: 0,
                    });
                    if rs.rows.len() >= limits.max_rows || kept >= limits.max_result_bytes {
                        rs.truncated = true;
                        continue;
                    }
                    kept += rs.push_cells(
                        (0..row.len()).map(|i| row.get(i).map(str::to_string)),
                        limits.max_cell_chars,
                    );
                }
                SimpleQueryMessage::CommandComplete(n) => match current.take() {
                    Some(rs) => {
                        out.info(if rs.truncated {
                            format!("({} rows shown of {n}, result truncated)", rs.rows.len())
                        } else {
                            format!("({n} row{} returned)", if n == 1 { "" } else { "s" })
                        });
                        out.result_sets.push(rs);
                    }
                    None => {
                        out.rows_affected += n;
                        out.info(format!("({n} row{} affected)", if n == 1 { "" } else { "s" }))
                    }
                },
                _ => {}
            }
        }
        if let Some(rs) = current.take() {
            out.result_sets.push(rs);
        }
        Ok(out)
    }

    /// The simple protocol returns text without column types; for a single
    /// statement, describe it (prepare, never run) to name them, so exports
    /// keep numbers as numbers and quote the rest.
    async fn add_types(&self, sql: &str, out: &mut BatchOutput) {
        if out.failed || out.result_sets.len() != 1 {
            return;
        }
        let pg = sqlotus_core::Dialect::Postgres;
        if sqlotus_core::statements::split_statements(sql, pg).len() != 1 {
            return;
        }
        // only statements that read: a failed describe inside the user's
        // transaction would abort it, and these described fine a moment ago
        let words: Vec<String> = sqlotus_core::tokenize(sql, pg)
            .iter()
            .filter(|t| t.kind == sqlotus_core::TokenKind::Word)
            .map(|t| t.upper())
            .collect();
        let reads = words
            .first()
            .is_some_and(|w| matches!(w.as_str(), "SELECT" | "WITH" | "VALUES" | "TABLE" | "SHOW" | "EXPLAIN"));
        if !reads && !words.iter().any(|w| w == "RETURNING") {
            return;
        }
        let Ok(stmt) = self.client.prepare(sql).await else {
            return;
        };
        let rs = &mut out.result_sets[0];
        let cols = stmt.columns();
        if cols.len() != rs.columns.len() || cols.iter().zip(&rs.columns).any(|(c, r)| c.name() != r.name) {
            return;
        }
        for (c, r) in cols.iter().zip(rs.columns.iter_mut()) {
            r.type_name = c.type_().name().to_string();
        }
    }
}

/// Why a `COPY … FROM STDIN` or `COPY … TO STDOUT` in `sql` cannot run here:
/// their rows travel between the server and a terminal client. The server
/// would wait for rows this connection cannot send, then end the session
/// (an open transaction with it), or send rows it cannot read.
fn client_copy(sql: &str) -> Option<&'static str> {
    let pg = sqlotus_core::Dialect::Postgres;
    for span in sqlotus_core::statements::split_statements(sql, pg) {
        // the statement's own words, not those of a query in parentheses
        let mut depth = 0;
        let mut words = Vec::new();
        for t in sqlotus_core::tokenize(span.text(sql), pg) {
            match t.kind {
                sqlotus_core::TokenKind::LParen => depth += 1,
                sqlotus_core::TokenKind::RParen => depth -= 1,
                sqlotus_core::TokenKind::Word if depth == 0 => words.push(t.upper()),
                _ => {}
            }
        }
        if words.first().map(String::as_str) != Some("COPY") {
            continue;
        }
        if words.windows(2).any(|w| w[0] == "FROM" && w[1] == "STDIN") {
            return Some(
                "COPY … FROM STDIN reads its rows from a terminal client, not from a script run here, so it was not run (the server would have ended the session). Insert the rows with INSERT, or COPY them from a file of the server.",
            );
        }
        if words.windows(2).any(|w| w[0] == "TO" && w[1] == "STDOUT") {
            return Some(
                "COPY … TO STDOUT writes its rows to a terminal client, which cannot show them here, so it was not run. Run the query itself to see the rows, then save them as CSV.",
            );
        }
    }
    None
}

fn pg_error(e: &tokio_postgres::Error) -> String {
    match e.as_db_error() {
        Some(db) => {
            let mut s = format!("{}: {}", db.severity(), db.message());
            if let Some(d) = db.detail() {
                s.push_str(&format!("\nDETAIL: {d}"));
            }
            if let Some(h) = db.hint() {
                s.push_str(&format!("\nHINT: {h}"));
            }
            if let Some(tokio_postgres::error::ErrorPosition::Original(p)) = db.position() {
                s.push_str(&format!("\nPosition: {p}"));
            }
            s
        }
        None => e.to_string(),
    }
}

#[cfg(test)]
mod tests {
    use super::client_copy;

    #[test]
    fn copy_through_a_terminal_client_is_refused() {
        assert!(client_copy("COPY t FROM STDIN").unwrap().contains("FROM STDIN"));
        assert!(client_copy("SELECT 1;\ncopy t (a, b) from stdin with (format csv);").is_some());
        assert!(
            client_copy("COPY (SELECT * FROM t) TO STDOUT WITH CSV HEADER")
                .unwrap()
                .contains("TO STDOUT")
        );
        // files of the server, a table named stdin in the query, text, comments
        assert_eq!(client_copy("COPY t FROM '/tmp/t.csv'"), None);
        assert_eq!(client_copy("COPY (SELECT * FROM stdin) TO '/tmp/x.csv'"), None);
        assert_eq!(client_copy("SELECT 'COPY t FROM STDIN'"), None);
        assert_eq!(client_copy("-- COPY t FROM STDIN\nSELECT 1"), None);
        assert_eq!(client_copy("DO $$ BEGIN COPY t FROM STDIN; END $$"), None);
    }
}
