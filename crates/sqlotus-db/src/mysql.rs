//! MySQL / MariaDB via mysql_async, using the text protocol.

use mysql_async::prelude::Queryable;
use mysql_async::{Conn, Opts, OptsBuilder, SslOpts, Value};

use crate::{BatchOutput, ConnectionConfig, DbError, Limits, Result, ResultColumn, ResultSet};

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
        let endpoint = cfg.endpoint();
        let mut b = OptsBuilder::default()
            .ip_or_hostname(endpoint.host)
            .tcp_port(endpoint.port.unwrap_or(cfg.default_port()))
            .user(cfg.user.clone())
            .pass(password.map(str::to_string))
            // report rows matched, like SQL Server and Postgres, not only rows whose value changed
            .client_found_rows(true)
            .db_name(cfg.database.clone().filter(|d| !d.is_empty()))
            .prefer_socket(false);
        if cfg.read_only {
            // the server itself refuses writes, also inside procedures (also after a reconnect)
            b = b.init(vec!["SET SESSION TRANSACTION READ ONLY"]);
        }
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
                    let shapes: Vec<Shape> = cols.iter().map(shape).collect();
                    let mut rs = ResultSet {
                        columns: cols
                            .iter()
                            .map(|c| ResultColumn {
                                name: c.name_str().to_string(),
                                type_name: type_name(c).into(),
                            })
                            .collect(),
                        rows: Vec::new(),
                        truncated: false,
                        clipped: 0,
                    };
                    loop {
                        match result.next().await {
                            Ok(Some(row)) => {
                                if rs.rows.len() >= limits.max_rows {
                                    rs.truncated = true;
                                    continue;
                                }
                                let cells = row.unwrap().into_iter().zip(&shapes).map(|(v, s)| value(v, *s));
                                rs.push_cells(cells, limits.max_cell_chars);
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

/// How a column's values are shown.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Shape {
    Text,
    /// BINARY, VARBINARY, BLOB, GEOMETRY: always `0x...`, even when the
    /// bytes happen to be valid UTF-8.
    Binary,
    /// BIT(n): the number (`b'1'` is 1, `b'10101010'` is 170), which is
    /// also what an edit writes back.
    Bit,
}

fn shape(c: &mysql_async::Column) -> Shape {
    use mysql_async::consts::ColumnType as T;
    match c.column_type() {
        T::MYSQL_TYPE_BIT => Shape::Bit,
        // charset 63 is "binary"; text columns carry the connection's charset
        T::MYSQL_TYPE_STRING
        | T::MYSQL_TYPE_VAR_STRING
        | T::MYSQL_TYPE_VARCHAR
        | T::MYSQL_TYPE_BLOB
        | T::MYSQL_TYPE_TINY_BLOB
        | T::MYSQL_TYPE_MEDIUM_BLOB
        | T::MYSQL_TYPE_LONG_BLOB
        | T::MYSQL_TYPE_GEOMETRY
            if c.character_set() == 63 =>
        {
            Shape::Binary
        }
        _ => Shape::Text,
    }
}

/// The SQL name of a column's type, as SQL Server and PostgreSQL results
/// give it (exports decide numbers and binary values by it).
fn type_name(c: &mysql_async::Column) -> &'static str {
    use mysql_async::consts::ColumnType as T;
    let binary = c.character_set() == 63;
    match c.column_type() {
        T::MYSQL_TYPE_TINY => "tinyint",
        T::MYSQL_TYPE_SHORT => "smallint",
        T::MYSQL_TYPE_INT24 => "mediumint",
        T::MYSQL_TYPE_LONG => "int",
        T::MYSQL_TYPE_LONGLONG => "bigint",
        T::MYSQL_TYPE_DECIMAL | T::MYSQL_TYPE_NEWDECIMAL => "decimal",
        T::MYSQL_TYPE_FLOAT => "float",
        T::MYSQL_TYPE_DOUBLE => "double",
        T::MYSQL_TYPE_BIT => "bit",
        T::MYSQL_TYPE_YEAR => "year",
        T::MYSQL_TYPE_DATE | T::MYSQL_TYPE_NEWDATE => "date",
        T::MYSQL_TYPE_TIME | T::MYSQL_TYPE_TIME2 => "time",
        T::MYSQL_TYPE_DATETIME | T::MYSQL_TYPE_DATETIME2 => "datetime",
        T::MYSQL_TYPE_TIMESTAMP | T::MYSQL_TYPE_TIMESTAMP2 => "timestamp",
        T::MYSQL_TYPE_JSON => "json",
        T::MYSQL_TYPE_ENUM => "enum",
        T::MYSQL_TYPE_SET => "set",
        T::MYSQL_TYPE_GEOMETRY => "geometry",
        T::MYSQL_TYPE_VECTOR => "vector",
        T::MYSQL_TYPE_NULL => "null",
        T::MYSQL_TYPE_STRING if binary => "binary",
        T::MYSQL_TYPE_STRING => "char",
        T::MYSQL_TYPE_VAR_STRING | T::MYSQL_TYPE_VARCHAR if binary => "varbinary",
        T::MYSQL_TYPE_VAR_STRING | T::MYSQL_TYPE_VARCHAR => "varchar",
        T::MYSQL_TYPE_TINY_BLOB | T::MYSQL_TYPE_BLOB | T::MYSQL_TYPE_MEDIUM_BLOB | T::MYSQL_TYPE_LONG_BLOB
            if binary =>
        {
            "blob"
        }
        T::MYSQL_TYPE_TINY_BLOB | T::MYSQL_TYPE_BLOB | T::MYSQL_TYPE_MEDIUM_BLOB | T::MYSQL_TYPE_LONG_BLOB => "text",
        _ => "",
    }
}

fn hex(bytes: &[u8]) -> String {
    let mut s = String::with_capacity(2 + bytes.len() * 2);
    s.push_str("0x");
    for x in bytes {
        s.push_str(&format!("{x:02X}"));
    }
    s
}

fn value(v: Value, shape: Shape) -> Option<String> {
    Some(match v {
        Value::NULL => return None,
        Value::Bytes(b) if shape == Shape::Binary => hex(&b),
        Value::Bytes(b) if shape == Shape::Bit && b.len() <= 8 => {
            b.iter().fold(0u64, |n, x| (n << 8) | u64::from(*x)).to_string()
        }
        Value::Bytes(b) => String::from_utf8(b).unwrap_or_else(|e| hex(e.as_bytes())),
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
