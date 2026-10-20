//! SQL Server via tiberius.

use chrono::{DateTime, FixedOffset, NaiveDate, NaiveDateTime, NaiveTime};
use futures_util::TryStreamExt;
use tiberius::{
    AuthMethod, Client, ColumnData, ColumnType, Config, EncryptionLevel, FromSql, QueryItem,
    SqlBrowser,
};
use tokio::net::TcpStream;
use tokio_util::compat::{Compat, TokioAsyncWriteCompatExt};

use crate::{
    AuthKind, BatchOutput, ConnectionConfig, DbError, Limits, Result, ResultColumn, ResultSet, clip,
};

pub struct MssqlConn {
    client: Client<Compat<TcpStream>>,
}

/// Split `host\instance` / `host,port`; `.` and `(local)` mean localhost.
pub(crate) fn parse_host(raw: &str) -> (String, Option<String>, Option<u16>) {
    let raw = raw.trim().trim_start_matches("tcp:");
    let (rest, port) = match raw.rsplit_once(',') {
        Some((h, p)) => (h, p.trim().parse().ok()),
        None => (raw, None),
    };
    let (host, instance) = match rest.split_once('\\') {
        Some((h, i)) => (h, Some(i.to_string())),
        None => (rest, None),
    };
    let host = match host.trim() {
        "." | "(local)" | "(localdb)" | "" => "localhost".to_string(),
        h => h.to_string(),
    };
    (host, instance, port)
}

impl MssqlConn {
    pub async fn open(cfg: &ConnectionConfig, password: Option<&str>) -> Result<Self> {
        let (host, instance, port) = parse_host(&cfg.host);
        let mut config = Config::new();
        config.host(&host);
        let port = cfg.port.or(port);
        if let Some(p) = port {
            config.port(p);
        }
        if let Some(i) = &instance {
            config.instance_name(i);
        }
        if let Some(db) = cfg.database.as_deref().filter(|d| !d.is_empty()) {
            config.database(db);
        }
        config.application_name("SqlSupportQuery");
        match cfg.auth {
            AuthKind::Windows => config.authentication(AuthMethod::Integrated),
            AuthKind::Sql => config.authentication(AuthMethod::sql_server(
                cfg.user.as_deref().unwrap_or_default(),
                password.unwrap_or_default(),
            )),
        }
        if cfg.encrypt {
            config.encryption(EncryptionLevel::Required);
            if cfg.trust_server_certificate {
                config.trust_cert();
            }
        } else {
            // Login-only encryption: like SqlClient with Encrypt=false, the
            // certificate is not validated.
            config.encryption(EncryptionLevel::Off);
            config.trust_cert();
        }

        let client = Self::connect(config, instance.is_some() && port.is_none()).await?;
        Ok(Self { client })
    }

    async fn connect(config: Config, named: bool) -> Result<Client<Compat<TcpStream>>> {
        let tcp = if named {
            TcpStream::connect_named(&config).await
        } else {
            TcpStream::connect(config.get_addr())
                .await
                .map_err(tiberius::error::Error::from)
        }
        .map_err(|e| DbError::Connect(e.to_string()))?;
        tcp.set_nodelay(true).ok();
        match Client::connect(config.clone(), tcp.compat_write()).await {
            Ok(c) => Ok(c),
            // Azure SQL gateway redirect
            Err(tiberius::error::Error::Routing { host, port }) => {
                let mut config = config;
                config.host(&host);
                config.port(port);
                let tcp = TcpStream::connect(config.get_addr())
                    .await
                    .map_err(|e| DbError::Connect(e.to_string()))?;
                tcp.set_nodelay(true).ok();
                Client::connect(config, tcp.compat_write())
                    .await
                    .map_err(|e| DbError::Connect(error_text(&e)))
            }
            Err(e) => Err(DbError::Connect(error_text(&e))),
        }
    }

    pub async fn execute(&mut self, sql: &str, limits: &Limits) -> BatchOutput {
        let mut out = BatchOutput::default();
        let mut stream = match self.client.simple_query(sql).await {
            Ok(s) => s,
            Err(e) => {
                out.error(error_text(&e));
                return out;
            }
        };
        let mut current: Option<ResultSet> = None;
        loop {
            match stream.try_next().await {
                Ok(Some(QueryItem::Metadata(meta))) => {
                    if let Some(rs) = current.take() {
                        out.result_sets.push(rs);
                    }
                    let columns = meta
                        .columns()
                        .iter()
                        .map(|c| ResultColumn {
                            name: c.name().to_string(),
                            type_name: type_name(c.column_type()).into(),
                        })
                        .collect();
                    current = Some(ResultSet {
                        columns,
                        rows: Vec::new(),
                        truncated: false,
                    });
                }
                Ok(Some(QueryItem::Row(row))) => {
                    if let Some(rs) = current.as_mut() {
                        if rs.rows.len() >= limits.max_rows {
                            rs.truncated = true;
                            continue;
                        }
                        rs.rows.push(
                            row.into_iter()
                                .map(|cd| cell(cd).map(|s| clip(s, limits.max_cell_chars)))
                                .collect(),
                        );
                    }
                }
                Ok(None) => break,
                Err(e) => {
                    out.error(error_text(&e));
                    break;
                }
            }
        }
        if let Some(rs) = current.take() {
            out.result_sets.push(rs);
        }
        if !out.failed {
            let counts: Vec<String> = out
                .result_sets
                .iter()
                .map(|rs| {
                    let n = rs.rows.len();
                    if rs.truncated {
                        format!("({n} rows shown, result truncated)")
                    } else {
                        format!("({n} row{} affected)", if n == 1 { "" } else { "s" })
                    }
                })
                .collect();
            for c in counts {
                out.info(c);
            }
            if out.result_sets.is_empty() {
                out.info("Commands completed successfully.");
            }
        }
        out
    }

    pub async fn close(self) {
        let _ = self.client.close().await;
    }
}

fn error_text(e: &tiberius::error::Error) -> String {
    match e {
        tiberius::error::Error::Server(t) => {
            let mut s = format!("Msg {}, Level {}, State {}", t.code(), t.class(), t.state());
            if !t.procedure().is_empty() {
                s.push_str(&format!(", Procedure {}", t.procedure()));
            }
            s.push_str(&format!(", Line {}\n{}", t.line(), t.message()));
            s
        }
        other => other.to_string(),
    }
}

fn type_name(t: ColumnType) -> &'static str {
    match t {
        ColumnType::Null => "null",
        ColumnType::Bit | ColumnType::Bitn => "bit",
        ColumnType::Int1 => "tinyint",
        ColumnType::Int2 => "smallint",
        ColumnType::Int4 => "int",
        ColumnType::Int8 => "bigint",
        ColumnType::Intn => "int",
        ColumnType::Datetime4 => "smalldatetime",
        ColumnType::Datetime | ColumnType::Datetimen => "datetime",
        ColumnType::Float4 => "real",
        ColumnType::Float8 | ColumnType::Floatn => "float",
        ColumnType::Money | ColumnType::Money4 => "money",
        ColumnType::Guid => "uniqueidentifier",
        ColumnType::Decimaln => "decimal",
        ColumnType::Numericn => "numeric",
        ColumnType::Daten => "date",
        ColumnType::Timen => "time",
        ColumnType::Datetime2 => "datetime2",
        ColumnType::DatetimeOffsetn => "datetimeoffset",
        ColumnType::BigVarBin => "varbinary",
        ColumnType::BigVarChar => "varchar",
        ColumnType::BigBinary => "binary",
        ColumnType::BigChar => "char",
        ColumnType::NVarchar => "nvarchar",
        ColumnType::NChar => "nchar",
        ColumnType::Xml => "xml",
        ColumnType::Udt => "udt",
        ColumnType::Text => "text",
        ColumnType::Image => "image",
        ColumnType::NText => "ntext",
        ColumnType::SSVariant => "sql_variant",
    }
}

fn hex(bytes: &[u8]) -> String {
    let mut s = String::with_capacity(2 + bytes.len() * 2);
    s.push_str("0x");
    for b in bytes {
        s.push_str(&format!("{b:02X}"));
    }
    s
}

/// Render a value the way SSMS's grid shows it.
fn cell(cd: ColumnData<'static>) -> Option<String> {
    Some(match &cd {
        ColumnData::U8(v) => v.as_ref()?.to_string(),
        ColumnData::I16(v) => v.as_ref()?.to_string(),
        ColumnData::I32(v) => v.as_ref()?.to_string(),
        ColumnData::I64(v) => v.as_ref()?.to_string(),
        ColumnData::F32(v) => v.as_ref()?.to_string(),
        ColumnData::F64(v) => v.as_ref()?.to_string(),
        ColumnData::Bit(v) => if (*v)? { "1" } else { "0" }.to_string(),
        ColumnData::String(v) => v.as_ref()?.to_string(),
        ColumnData::Guid(v) => v.as_ref()?.to_string().to_uppercase(),
        ColumnData::Binary(v) => hex(v.as_ref()?),
        ColumnData::Numeric(v) => v.as_ref()?.to_string(),
        ColumnData::Xml(v) => v.as_ref()?.to_string(),
        ColumnData::DateTime(None)
        | ColumnData::SmallDateTime(None)
        | ColumnData::DateTime2(None)
        | ColumnData::Date(None)
        | ColumnData::Time(None)
        | ColumnData::DateTimeOffset(None) => return None,
        ColumnData::DateTime(_) => NaiveDateTime::from_sql(&cd)
            .ok()??
            .format("%Y-%m-%d %H:%M:%S%.3f")
            .to_string(),
        ColumnData::SmallDateTime(_) => NaiveDateTime::from_sql(&cd)
            .ok()??
            .format("%Y-%m-%d %H:%M:%S")
            .to_string(),
        ColumnData::DateTime2(_) => NaiveDateTime::from_sql(&cd)
            .ok()??
            .format("%Y-%m-%d %H:%M:%S%.f")
            .to_string(),
        ColumnData::Date(_) => NaiveDate::from_sql(&cd)
            .ok()??
            .format("%Y-%m-%d")
            .to_string(),
        ColumnData::Time(_) => NaiveTime::from_sql(&cd)
            .ok()??
            .format("%H:%M:%S%.f")
            .to_string(),
        ColumnData::DateTimeOffset(_) => DateTime::<FixedOffset>::from_sql(&cd)
            .ok()??
            .format("%Y-%m-%d %H:%M:%S%.f %:z")
            .to_string(),
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn host_parsing() {
        assert_eq!(
            parse_host(r".\SQLEXPRESS"),
            ("localhost".into(), Some("SQLEXPRESS".into()), None)
        );
        assert_eq!(
            parse_host("db.example.com,1500"),
            ("db.example.com".into(), None, Some(1500))
        );
        assert_eq!(parse_host("tcp:10.0.0.5"), ("10.0.0.5".into(), None, None));
    }
}
