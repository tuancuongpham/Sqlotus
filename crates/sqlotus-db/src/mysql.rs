//! MySQL / MariaDB via mysql_async, using the text protocol.

use mysql_async::prelude::Queryable;
use mysql_async::{Conn, Opts, OptsBuilder, SslOpts, Value};

use crate::{BatchOutput, ConnectionConfig, DbError, Limits, Result, ResultColumn, ResultSet, clip};

pub struct MySqlConn {
    conn: Conn,
    opts: Opts,
}

#[derive(Clone)]
pub struct MySqlCancel {
    opts: Opts,
    id: u32,
}

impl MySqlCancel {
    /// `KILL QUERY` from a second connection.
    pub async fn cancel(&self) -> Result<()> {
        let mut c = Conn::new(self.opts.clone())
            .await
            .map_err(|e| DbError::Query(e.to_string()))?;
        c.query_drop(format!("KILL QUERY {}", self.id))
            .await
            .map_err(|e| DbError::Query(e.to_string()))?;
        c.disconnect().await.ok();
        Ok(())
    }
}

impl MySqlConn {
    /// Whether a transaction is open, from the server status flags of the
    /// last OK packet (information_schema.innodb_trx also lists the query
    /// that asks).
    pub async fn in_transaction(&mut self) -> Result<bool> {
        self.conn
            .query_drop("DO 0")
            .await
            .map_err(|e| DbError::Query(e.to_string()))?;
        Ok(self.conn.last_ok_packet().is_some_and(|ok| {
            ok.status_flags()
                .contains(mysql_async::consts::StatusFlags::SERVER_STATUS_IN_TRANS)
        }))
    }

    pub async fn open(cfg: &ConnectionConfig, password: Option<&str>) -> Result<Self> {
        let mut b = OptsBuilder::default()
            .ip_or_hostname(cfg.host.trim())
            .tcp_port(cfg.port.unwrap_or(3306))
            .user(cfg.user.clone())
            .pass(password.map(str::to_string))
            // report rows matched, like SQL Server and Postgres, not only rows whose value changed
            .client_found_rows(true)
            .db_name(cfg.database.clone().filter(|d| !d.is_empty()))
            .prefer_socket(false);
        if cfg.encrypt {
            b = b.ssl_opts(
                SslOpts::default()
                    .with_danger_accept_invalid_certs(cfg.trust_server_certificate)
                    .with_danger_skip_domain_validation(cfg.trust_server_certificate),
            );
        }
        let opts: Opts = b.into();
        let conn = Conn::new(opts.clone())
            .await
            .map_err(|e| DbError::Connect(e.to_string()))?;
        Ok(Self { conn, opts })
    }

    pub fn cancel_handle(&self) -> MySqlCancel {
        MySqlCancel {
            opts: self.opts.clone(),
            id: self.conn.id(),
        }
    }

    pub async fn execute(&mut self, sql: &str, limits: &Limits) -> BatchOutput {
        let mut out = BatchOutput::default();
        let mut result = match self.conn.query_iter(sql).await {
            Ok(r) => r,
            Err(e) => {
                out.error(e.to_string());
                return out;
            }
        };
        loop {
            let columns = result.columns();
            match columns {
                Some(cols) if !cols.is_empty() => {
                    let mut rs = ResultSet {
                        columns: cols
                            .iter()
                            .map(|c| ResultColumn {
                                name: c.name_str().to_string(),
                                type_name: format!("{:?}", c.column_type()),
                            })
                            .collect(),
                        rows: Vec::new(),
                        truncated: false,
                    };
                    loop {
                        match result.next().await {
                            Ok(Some(row)) => {
                                if rs.rows.len() >= limits.max_rows {
                                    rs.truncated = true;
                                    continue;
                                }
                                rs.rows.push(
                                    row.unwrap()
                                        .into_iter()
                                        .map(|v| value(v).map(|s| clip(s, limits.max_cell_chars)))
                                        .collect(),
                                );
                            }
                            Ok(None) => break,
                            Err(e) => {
                                out.error(e.to_string());
                                return out;
                            }
                        }
                    }
                    let n = rs.rows.len();
                    out.info(format!("({n} row{} returned)", if n == 1 { "" } else { "s" }));
                    out.result_sets.push(rs);
                }
                _ => {
                    let n = result.affected_rows();
                    out.rows_affected += n;
                    out.info(format!("({n} row{} affected)", if n == 1 { "" } else { "s" }));
                    // consume the empty set and move to the next one
                    if let Err(e) = result.next().await {
                        out.error(e.to_string());
                        return out;
                    }
                }
            }
            if result.is_empty() {
                break;
            }
        }
        out
    }
}

fn value(v: Value) -> Option<String> {
    Some(match v {
        Value::NULL => return None,
        Value::Bytes(b) => String::from_utf8(b).unwrap_or_else(|e| {
            let bytes = e.into_bytes();
            let mut s = String::from("0x");
            for x in bytes {
                s.push_str(&format!("{x:02X}"));
            }
            s
        }),
        Value::Int(i) => i.to_string(),
        Value::UInt(u) => u.to_string(),
        Value::Float(f) => f.to_string(),
        Value::Double(d) => d.to_string(),
        Value::Date(y, mo, d, h, mi, s, us) => {
            if us > 0 {
                format!("{y:04}-{mo:02}-{d:02} {h:02}:{mi:02}:{s:02}.{us:06}")
            } else {
                format!("{y:04}-{mo:02}-{d:02} {h:02}:{mi:02}:{s:02}")
            }
        }
        Value::Time(neg, days, h, m, s, us) => {
            let sign = if neg { "-" } else { "" };
            let h = days * 24 + h as u32;
            if us > 0 {
                format!("{sign}{h:02}:{m:02}:{s:02}.{us:06}")
            } else {
                format!("{sign}{h:02}:{m:02}:{s:02}")
            }
        }
    })
}
