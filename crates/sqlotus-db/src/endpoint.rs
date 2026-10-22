//! Where a connection goes: the Host field as people type it, read the way
//! each server's own tools read it. SQL Server takes `host\instance`,
//! `host,port` and `tcp:host`, with `.` / `(local)` for this computer;
//! PostgreSQL and MySQL take `host:port` and `[ipv6]:port`. A number in the
//! Port field wins over one typed in the host.

use sqlotus_core::Dialect;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Endpoint {
    pub host: String,
    /// SQL Server named instance, found through the SQL Server Browser.
    pub instance: Option<String>,
    /// `None`: the server's default port (or the named instance's).
    pub port: Option<u16>,
}

impl Endpoint {
    pub fn parse(dialect: Dialect, raw: &str) -> Self {
        let raw = raw.trim();
        let mut e = match dialect {
            Dialect::MsSql => Self::sql_server(raw),
            Dialect::Postgres | Dialect::MySql => Self::host_port(raw),
        };
        if e.host.is_empty() {
            e.host = "localhost".into();
        }
        e
    }

    fn sql_server(raw: &str) -> Self {
        let raw = match raw.get(..4) {
            Some(p) if p.eq_ignore_ascii_case("tcp:") => &raw[4..],
            _ => raw,
        };
        let (rest, port) = match raw.rsplit_once(',') {
            Some((h, p)) => (h, p.trim().parse().ok()),
            None => (raw, None),
        };
        let (host, instance) = match rest.split_once('\\') {
            Some((h, i)) => (h.trim(), Some(i.trim()).filter(|i| !i.is_empty())),
            None => (rest.trim(), None),
        };
        let host = match host.to_ascii_lowercase().as_str() {
            "." | "(local)" | "(localdb)" => "localhost".to_string(),
            _ => host.to_string(),
        };
        Self {
            host,
            instance: instance.map(str::to_string),
            port,
        }
    }

    fn host_port(raw: &str) -> Self {
        let plain = |host: &str, port| Self {
            host: host.trim().to_string(),
            instance: None,
            port,
        };
        // [::1] or [::1]:5432
        if let Some((addr, tail)) = raw.strip_prefix('[').and_then(|r| r.split_once(']')) {
            return plain(addr, tail.strip_prefix(':').and_then(|p| p.trim().parse().ok()));
        }
        // host:port; more than one colon is a bare IPv6 address
        match raw.split_once(':') {
            Some((host, port)) if !port.contains(':') => match port.trim().parse() {
                Ok(p) => plain(host, Some(p)),
                Err(_) => plain(raw, None),
            },
            _ => plain(raw, None),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn parse(d: Dialect, raw: &str) -> (String, Option<String>, Option<u16>) {
        let e = Endpoint::parse(d, raw);
        (e.host, e.instance, e.port)
    }

    #[test]
    fn sql_server_names() {
        let ms = |raw| parse(Dialect::MsSql, raw);
        assert_eq!(
            ms(r".\SQLEXPRESS"),
            ("localhost".into(), Some("SQLEXPRESS".into()), None)
        );
        assert_eq!(
            ms(r"(LOCAL)\SQLEXPRESS,1500"),
            ("localhost".into(), Some("SQLEXPRESS".into()), Some(1500))
        );
        assert_eq!(ms("db.example.com,1500"), ("db.example.com".into(), None, Some(1500)));
        assert_eq!(ms("tcp:10.0.0.5"), ("10.0.0.5".into(), None, None));
        assert_eq!(ms("TCP:10.0.0.5, 1500"), ("10.0.0.5".into(), None, Some(1500)));
        assert_eq!(ms(r"db\"), ("db".into(), None, None));
        assert_eq!(ms("  "), ("localhost".into(), None, None));
    }

    #[test]
    fn host_and_port() {
        for d in [Dialect::Postgres, Dialect::MySql] {
            let p = |raw| parse(d, raw);
            assert_eq!(p("db.example.com"), ("db.example.com".into(), None, None));
            assert_eq!(p(" db.example.com:5433 "), ("db.example.com".into(), None, Some(5433)));
            assert_eq!(p("10.0.0.5:3307"), ("10.0.0.5".into(), None, Some(3307)));
            assert_eq!(p("::1"), ("::1".into(), None, None));
            assert_eq!(p("fe80::1:2"), ("fe80::1:2".into(), None, None));
            assert_eq!(p("[::1]"), ("::1".into(), None, None));
            assert_eq!(p("[::1]:5433"), ("::1".into(), None, Some(5433)));
            // not a port: left for the server name check to report
            assert_eq!(p("db:abc"), ("db:abc".into(), None, None));
            assert_eq!(p(""), ("localhost".into(), None, None));
        }
    }
}
