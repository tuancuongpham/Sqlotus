//! PostgreSQL via tokio-postgres, using the simple (text) protocol so every
//! value arrives as text and multi-statement scripts work.

use std::sync::{Arc, Mutex};

use futures_util::StreamExt;
use postgres_native_tls::MakeTlsConnector;
use tokio_postgres::config::SslMode;
use tokio_postgres::{AsyncMessage, CancelToken, Client, SimpleQueryMessage};

use crate::{
    BatchOutput, ConnectionConfig, DbError, Limits, Result, ResultColumn, ResultSet, clip,
};

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
        let mut config = tokio_postgres::Config::new();
        config
            .host(cfg.host.trim())
            .port(cfg.port.unwrap_or(5432))
            .user(cfg.user.as_deref().unwrap_or("postgres"))
            .dbname(
                cfg.database
                    .as_deref()
                    .filter(|d| !d.is_empty())
                    .unwrap_or("postgres"),
            )
            .application_name("Sqlotus")
            .ssl_mode(if cfg.encrypt {
                SslMode::Require
            } else {
                SslMode::Prefer
            });
        if let Some(p) = password {
            config.password(p);
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
        Ok(Self {
            client,
            notices,
            tls,
        })
    }

    pub fn cancel_handle(&self) -> PgCancel {
        PgCancel {
            token: self.client.cancel_token(),
            tls: self.tls.clone(),
        }
    }

    pub async fn execute(&mut self, sql: &str, limits: &Limits) -> BatchOutput {
        let mut out = BatchOutput::default();
        let result = self.client.simple_query(sql).await;
        let notices: Vec<String> = self
            .notices
            .lock()
            .map(|mut v| std::mem::take(&mut *v))
            .unwrap_or_default();
        for n in notices {
            out.info(n);
        }
        let messages = match result {
            Ok(m) => m,
            Err(e) => {
                out.error(pg_error(&e));
                return out;
            }
        };
        let mut current: Option<ResultSet> = None;
        for msg in messages {
            match msg {
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
                    });
                    if rs.rows.len() >= limits.max_rows {
                        rs.truncated = true;
                        continue;
                    }
                    rs.rows.push(
                        (0..row.len())
                            .map(|i| {
                                row.get(i)
                                    .map(|s| clip(s.to_string(), limits.max_cell_chars))
                            })
                            .collect(),
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
                    None => out.info(format!(
                        "({n} row{} affected)",
                        if n == 1 { "" } else { "s" }
                    )),
                },
                _ => {}
            }
        }
        if let Some(rs) = current.take() {
            out.result_sets.push(rs);
        }
        out
    }
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
