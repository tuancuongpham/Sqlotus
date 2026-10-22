//! SQL Server via tiberius.

use chrono::{DateTime, FixedOffset, NaiveDate, NaiveDateTime, NaiveTime, Timelike};
use futures_util::TryStreamExt;
use tiberius::{
    AuthMethod, Client, ColumnData, ColumnType, Command, CommandItem, Config, EncryptionLevel, FromSql, QueryItem,
    ResultMetadata, Row, SqlBrowser,
};
use tokio::net::TcpStream;
use tokio_util::compat::{Compat, TokioAsyncWriteCompatExt};

use sqlotus_core::{Dialect, TokenKind, tokenize};

use crate::{AuthKind, BatchOutput, ConnectionConfig, DbError, Endpoint, Limits, Result, ResultColumn, ResultSet};

pub struct MssqlConn {
    client: Client<Compat<TcpStream>>,
}

/// The batch for [`MssqlConn::execute_counted`] with every row change
/// counted: NOCOUNT, set earlier in the session or by the script, hides the
/// counts that saving edited rows and the large-change check rely on. The
/// setting changes only inside the call; the session keeps its own.
fn counted_batch(sql: &str) -> String {
    let tokens = tokenize(sql, Dialect::MsSql);
    let words: Vec<_> = tokens.iter().filter(|t| !t.is_trivia()).collect();
    let is = |t: &sqlotus_core::Token<'_>, w: &str| t.kind == TokenKind::Word && t.text.eq_ignore_ascii_case(w);
    // on the first line, so error line numbers stay those of the script
    let mut out = String::from("SET NOCOUNT OFF; ");
    let mut at = 0;
    for w in words.windows(3) {
        if is(w[0], "SET") && is(w[1], "NOCOUNT") && is(w[2], "ON") {
            out.push_str(&sql[at..w[2].start]);
            out.push_str("OFF");
            at = w[2].end();
        }
    }
    out.push_str(&sql[at..]);
    out
}

impl MssqlConn {
    pub async fn open(cfg: &ConnectionConfig, password: Option<&str>) -> Result<Self> {
        let Endpoint { host, instance, port } = cfg.endpoint();
        let mut config = Config::new();
        config.host(&host);
        if let Some(p) = port {
            config.port(p);
        }
        if let Some(i) = &instance {
            config.instance_name(i);
        }
        if let Some(db) = cfg.database.as_deref().filter(|d| !d.is_empty()) {
            config.database(db);
        }
        config.application_name("Sqlotus");
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

    /// Run the batch through `sp_executesql`, which reports the rows each
    /// statement changed (plain batches do not). Variables, SET options and
    /// temporary tables created inside do not outlive the call, so this is
    /// only used for scripts that just change rows.
    pub async fn execute_counted(&mut self, sql: &str, limits: &Limits) -> BatchOutput {
        let mut out = BatchOutput::default();
        let mut cmd = Command::new("sp_executesql");
        cmd.bind_param("@stmt", counted_batch(sql));
        let mut stream = match cmd.exec(&mut self.client).await {
            Ok(s) => s,
            Err(e) => {
                out.error(error_text(&e));
                return out;
            }
        };
        let mut current: Option<ResultSet> = None;
        // the count after a result set belongs to that SELECT
        let mut after_select = false;
        loop {
            match stream.try_next().await {
                Ok(Some(CommandItem::Metadata(meta))) => {
                    out.result_sets.extend(current.replace(new_result_set(&meta)));
                    after_select = true;
                }
                Ok(Some(CommandItem::Row(row))) => {
                    if let Some(rs) = current.as_mut() {
                        push_row(rs, row, limits);
                    }
                }
                Ok(Some(CommandItem::RowsAffected(n))) => {
                    if after_select {
                        after_select = false;
                    } else if n > 0 {
                        out.rows_affected += n;
                        out.info(format!("({n} row{} affected)", if n == 1 { "" } else { "s" }));
                    }
                }
                Ok(Some(_)) => {}
                Ok(None) => break,
                Err(e) => {
                    out.error(error_text(&e));
                    break;
                }
            }
        }
        out.result_sets.extend(current.take());
        if !out.failed && out.result_sets.is_empty() && out.rows_affected == 0 {
            out.info("Commands completed successfully.");
        }
        out
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
                    out.result_sets.extend(current.replace(new_result_set(&meta)));
                }
                Ok(Some(QueryItem::Row(row))) => {
                    if let Some(rs) = current.as_mut() {
                        push_row(rs, row, limits);
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

fn new_result_set(meta: &ResultMetadata) -> ResultSet {
    ResultSet {
        columns: meta
            .columns()
            .iter()
            .map(|c| ResultColumn {
                name: c.name().to_string(),
                type_name: type_name(c.column_type()).into(),
            })
            .collect(),
        rows: Vec::new(),
        truncated: false,
        clipped: 0,
    }
}

fn push_row(rs: &mut ResultSet, row: Row, limits: &Limits) {
    if rs.rows.len() >= limits.max_rows {
        rs.truncated = true;
        return;
    }
    rs.push_cells(row.into_iter().map(cell), limits.max_cell_chars);
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

/// `.fffffff` with exactly `scale` digits, the usual display of datetime2/time.
fn fraction(nanos: u32, scale: u8) -> String {
    if scale == 0 {
        return String::new();
    }
    let digits = format!("{nanos:09}");
    format!(".{}", &digits[..(scale as usize).min(9)])
}

/// Render a value the way SQL Server tools usually show it.
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
        ColumnData::DateTime2(Some(v)) => {
            let dt = NaiveDateTime::from_sql(&cd).ok()??;
            format!(
                "{}{}",
                dt.format("%Y-%m-%d %H:%M:%S"),
                fraction(dt.and_utc().timestamp_subsec_nanos(), v.time().scale())
            )
        }
        ColumnData::Date(_) => NaiveDate::from_sql(&cd).ok()??.format("%Y-%m-%d").to_string(),
        ColumnData::Time(Some(v)) => {
            let t = NaiveTime::from_sql(&cd).ok()??;
            format!("{}{}", t.format("%H:%M:%S"), fraction(t.nanosecond(), v.scale()))
        }
        ColumnData::DateTimeOffset(Some(v)) => {
            let dt = DateTime::<FixedOffset>::from_sql(&cd).ok()??;
            let scale = v.datetime2().time().scale();
            format!(
                "{}{} {}",
                dt.format("%Y-%m-%d %H:%M:%S"),
                fraction(dt.timestamp_subsec_nanos(), scale),
                dt.format("%:z")
            )
        }
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn fractions() {
        assert_eq!(fraction(123_456_700, 7), ".1234567");
        assert_eq!(fraction(120_000_000, 3), ".120");
        assert_eq!(fraction(5, 0), "");
    }
}
