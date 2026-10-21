//! Connection Doctor: explain why a connection failed and offer fixes.
//!
//! Diagnosis classifies the driver error and, for servers on this computer,
//! inspects the Windows services and SQL Server network configuration
//! (read-only). Fixes that change the machine (enable TCP/IP, start a
//! service, ...) run elevated after a UAC prompt; fixes that only change the
//! connection settings are applied by the UI.

use std::net::ToSocketAddrs;
use std::process::Command;
use std::time::Duration;

use serde::{Deserialize, Serialize};
use sqlotus_core::Dialect;
use sqlotus_db::{AuthKind, Connection, ConnectionConfig};

use crate::commands::CmdResult;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum Level {
    Ok,
    Info,
    Warning,
    Error,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "camelCase")]
pub enum Fix {
    /// Enable the TCP/IP protocol of a SQL Server instance and restart it.
    #[serde(rename_all = "camelCase")]
    EnableTcp {
        instance_id: String,
        service: String,
    },
    /// Start a stopped Windows service.
    StartService {
        service: String,
    },
    /// Enable and start the SQL Server Browser (named instances).
    EnableBrowser,
    /// Allow SQL Server logins (mixed mode) and restart the instance.
    #[serde(rename_all = "camelCase")]
    EnableMixedAuth {
        instance_id: String,
        service: String,
    },
    /// Allow inbound TCP to the port (and UDP 1434 for the Browser) in
    /// Windows Firewall, so other computers can connect.
    OpenFirewall {
        port: u16,
        browser: bool,
    },
    /// `ALTER LOGIN … ENABLE`, run over a Windows Authentication connection
    /// to the same server.
    EnableLogin {
        login: String,
    },
    /// Connection setting changes, applied by the UI.
    UsePort {
        port: u16,
    },
    UseHost {
        host: String,
    },
    UseWindowsAuth,
    TrustCertificate,
    #[serde(rename_all = "camelCase")]
    UseDatabase {
        database: Option<String>,
    },
}

impl Fix {
    pub fn needs_admin(&self) -> bool {
        matches!(
            self,
            Fix::EnableTcp { .. }
                | Fix::StartService { .. }
                | Fix::EnableBrowser
                | Fix::EnableMixedAuth { .. }
                | Fix::OpenFirewall { .. }
        )
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Finding {
    pub level: Level,
    pub title: String,
    pub detail: String,
    pub fix: Option<Fix>,
    /// Label of the fix button.
    pub fix_label: Option<String>,
    pub needs_admin: bool,
}

impl Finding {
    fn new(level: Level, title: impl Into<String>, detail: impl Into<String>) -> Self {
        Self {
            level,
            title: title.into(),
            detail: detail.into(),
            fix: None,
            fix_label: None,
            needs_admin: false,
        }
    }

    fn with_fix(mut self, label: impl Into<String>, fix: Fix) -> Self {
        self.needs_admin = fix.needs_admin();
        self.fix_label = Some(label.into());
        self.fix = Some(fix);
        self
    }
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Diagnosis {
    pub summary: String,
    pub findings: Vec<Finding>,
}

// ---------------------------------------------------------------------------
// error classification
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Cause {
    Refused,
    Timeout,
    UnknownHost,
    InstanceNotFound,
    LoginFailed,
    Certificate,
    Database,
    WindowsAuth,
    Other,
}

pub fn classify(error: &str) -> Cause {
    let e = error.to_lowercase();
    let has = |needles: &[&str]| needles.iter().any(|n| e.contains(n));
    if has(&["certificate", "ssl", "tls", "handshake", "schannel"]) {
        Cause::Certificate
    } else if has(&[
        "18456",
        "login failed",
        "password authentication failed",
        "access denied for user",
        "authentication failed",
        "28p01",
        "1045",
    ]) {
        Cause::LoginFailed
    } else if has(&["sspi", "kerberos", "integrated", "untrusted domain", "18452"]) {
        Cause::WindowsAuth
    } else if has(&[
        "4060",
        "cannot open database",
        "does not exist",
        "unknown database",
        "3d000",
        "1049",
    ]) {
        Cause::Database
    } else if has(&["sql browser", "named instance", "instance", "ssrp", "error locating"]) {
        Cause::InstanceNotFound
    } else if has(&[
        "no such host",
        "not known",
        "failed to lookup",
        "name resolution",
        "11001",
        "dns",
    ]) {
        Cause::UnknownHost
    } else if has(&["refused", "10061", "connection reset", "10054"]) {
        Cause::Refused
    } else if has(&["timed out", "timeout", "10060"]) {
        Cause::Timeout
    } else {
        Cause::Other
    }
}

/// `host\instance,port` split, SQL Server style.
fn split_host(raw: &str) -> (String, Option<String>, Option<u16>) {
    let raw = raw.trim().trim_start_matches("tcp:");
    let (rest, port) = match raw.rsplit_once(',') {
        Some((h, p)) => (h, p.trim().parse().ok()),
        None => (raw, None),
    };
    match rest.split_once('\\') {
        Some((h, i)) => (h.trim().to_string(), Some(i.trim().to_string()), port),
        None => (rest.trim().to_string(), None, port),
    }
}

fn is_local(host: &str) -> bool {
    let h = host.to_lowercase();
    let machine = std::env::var("COMPUTERNAME").unwrap_or_default().to_lowercase();
    matches!(
        h.as_str(),
        "" | "." | "(local)" | "localhost" | "127.0.0.1" | "::1" | "(localdb)"
    ) || (!machine.is_empty() && (h == machine || h.starts_with(&format!("{machine}."))))
}

// ---------------------------------------------------------------------------
// local machine inspection (read-only PowerShell)
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Default, Deserialize)]
#[serde(default, rename_all = "camelCase")]
struct SqlInstance {
    name: String,
    id: String,
    service: String,
    status: String,
    tcp_enabled: i64,
    port: String,
    dynamic_port: String,
    login_mode: i64,
}

#[derive(Debug, Clone, Default, Deserialize)]
#[serde(default, rename_all = "camelCase")]
struct SqlServerState {
    instances: Vec<SqlInstance>,
    browser: String,
    browser_start: String,
}

const SQLSERVER_STATE_PS: &str = r#"
$ErrorActionPreference = 'SilentlyContinue'
$names = Get-ItemProperty 'HKLM:\SOFTWARE\Microsoft\Microsoft SQL Server\Instance Names\SQL'
$out = @()
if ($names) {
  foreach ($p in $names.PSObject.Properties | Where-Object { $_.Name -notlike 'PS*' }) {
    $base = "HKLM:\SOFTWARE\Microsoft\Microsoft SQL Server\$($p.Value)\MSSQLServer"
    $tcp = Get-ItemProperty "$base\SuperSocketNetLib\Tcp"
    $ipall = Get-ItemProperty "$base\SuperSocketNetLib\Tcp\IPAll"
    $svcName = if ($p.Name -eq 'MSSQLSERVER') { 'MSSQLSERVER' } else { 'MSSQL$' + $p.Name }
    $svc = Get-Service $svcName
    $out += [pscustomobject]@{
      name = $p.Name; id = $p.Value; service = $svcName; status = "$($svc.Status)"
      tcpEnabled = [int]$tcp.Enabled; port = "$($ipall.TcpPort)"; dynamicPort = "$($ipall.TcpDynamicPorts)"
      loginMode = [int](Get-ItemProperty $base).LoginMode
    }
  }
}
$br = Get-Service SQLBrowser
[pscustomobject]@{ instances = @($out); browser = "$($br.Status)"; browserStart = "$($br.StartType)" } | ConvertTo-Json -Depth 4 -Compress
"#;

fn powershell(script: &str) -> Option<String> {
    let mut cmd = Command::new("powershell");
    cmd.args([
        "-NoProfile",
        "-NonInteractive",
        "-ExecutionPolicy",
        "Bypass",
        "-Command",
        script,
    ]);
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        cmd.creation_flags(0x0800_0000); // CREATE_NO_WINDOW
    }
    let out = cmd.output().ok()?;
    Some(String::from_utf8_lossy(&out.stdout).trim().to_string())
}

fn sqlserver_state() -> Option<SqlServerState> {
    serde_json::from_str(&powershell(SQLSERVER_STATE_PS)?).ok()
}

/// Status of Windows services whose name matches `pattern` (e.g. postgresql*).
fn services_like(pattern: &str) -> Vec<(String, String)> {
    let script = format!(
        "Get-Service -Name '{pattern}' -ErrorAction SilentlyContinue | ForEach-Object {{ \"$($_.Name)|$($_.Status)\" }}"
    );
    powershell(&script)
        .unwrap_or_default()
        .lines()
        .filter_map(|l| {
            l.split_once('|')
                .map(|(n, s)| (n.trim().to_string(), s.trim().to_string()))
        })
        .collect()
}

/// Whether Windows Firewall has an enabled inbound allow rule for the TCP
/// port or for sqlservr.exe. `None` when it cannot be read.
fn firewall_allows(port: u16) -> Option<bool> {
    let script = format!(
        r#"$ErrorActionPreference = 'SilentlyContinue'
$allow = {{ $_.Enabled -eq 'True' -and $_.Direction -eq 'Inbound' -and $_.Action -eq 'Allow' }}
$byPort = Get-NetFirewallPortFilter -Protocol TCP | Where-Object {{ @($_.LocalPort) -contains '{port}' }} | Get-NetFirewallRule | Where-Object $allow
$byApp = Get-NetFirewallApplicationFilter | Where-Object {{ $_.Program -like '*sqlservr.exe' }} | Get-NetFirewallRule | Where-Object $allow
if ($byPort -or $byApp) {{ 'open' }} else {{ 'closed' }}"#
    );
    match powershell(&script)?.as_str() {
        "open" => Some(true),
        "closed" => Some(false),
        _ => None,
    }
}

/// The reason SQL Server logged for the latest failed login (event 18456 in
/// the Windows Application log, readable without administrator rights).
fn recent_login_failure(service: &str) -> Option<(String, String)> {
    let script = format!(
        "$e = Get-WinEvent -FilterHashtable @{{LogName='Application'; Id=18456; StartTime=(Get-Date).AddMinutes(-30)}} -MaxEvents 20 -ErrorAction SilentlyContinue | Where-Object {{ $_.ProviderName -eq {} }} | Select-Object -First 1\nif ($e) {{ $e.Message }}",
        ps_quote(service)
    );
    parse_login_failure(&powershell(&script)?)
}

/// `Login failed for user 'sa'. Reason: Password did not match … [CLIENT: …]`
/// → (user, reason).
fn parse_login_failure(message: &str) -> Option<(String, String)> {
    let user = message.split('\'').nth(1).unwrap_or("").to_string();
    let reason = message.split_once("Reason:")?.1;
    let reason = reason
        .split(" [CLIENT")
        .next()
        .unwrap_or(reason)
        .trim()
        .trim_end_matches('.')
        .to_string();
    Some((user, reason))
}

/// Findings for a login failure reason logged by SQL Server.
fn login_failure_findings(user: &str, reason: &str, local_windows: bool) -> Vec<Finding> {
    let r = reason.to_lowercase();
    let windows = |f: Finding| {
        if local_windows {
            f.with_fix("Use Windows Authentication", Fix::UseWindowsAuth)
        } else {
            f
        }
    };
    if r.contains("password did not match") {
        vec![windows(Finding::new(
            Level::Error,
            format!("Wrong password for login '{user}'"),
            format!("SQL Server says: {reason}. Passwords are case-sensitive."),
        ))]
    } else if r.contains("could not find a login") {
        vec![windows(Finding::new(
            Level::Error,
            format!("Login '{user}' does not exist on this server"),
            format!("SQL Server says: {reason}. Check the user name, or create the login."),
        ))]
    } else if r.contains("disabled") {
        let mut f = Finding::new(
            Level::Error,
            format!("Login '{user}' is disabled"),
            format!("SQL Server says: {reason}."),
        );
        if local_windows {
            f = f.with_fix(
                format!("Enable login {user}"),
                Fix::EnableLogin {
                    login: user.to_string(),
                },
            );
            f.detail
                .push_str(" Sqlotus can enable it through your Windows login if you are a sysadmin.");
        }
        vec![f]
    } else if r.contains("expired") || r.contains("must be changed") {
        vec![Finding::new(
            Level::Error,
            format!("The password of '{user}' has expired or must be changed"),
            format!(
                "SQL Server says: {reason}. Change it with ALTER LOGIN [{user}] WITH PASSWORD = '…' from an administrator connection."
            ),
        )]
    } else if r.contains("database") {
        vec![
            Finding::new(
                Level::Error,
                "The login's database could not be opened",
                format!("SQL Server says: {reason}."),
            )
            .with_fix("Connect to the default database", Fix::UseDatabase { database: None }),
        ]
    } else {
        vec![Finding::new(
            Level::Error,
            format!("SQL Server rejected login '{user}'"),
            format!("SQL Server says: {reason}."),
        )]
    }
}

fn port_open(host: &str, port: u16) -> bool {
    let target = if host.is_empty() || host == "." || host == "(local)" {
        "127.0.0.1"
    } else {
        host
    };
    match (target, port).to_socket_addrs() {
        Ok(addrs) => addrs
            .into_iter()
            .any(|a| std::net::TcpStream::connect_timeout(&a, Duration::from_millis(1500)).is_ok()),
        Err(_) => false,
    }
}

// ---------------------------------------------------------------------------
// diagnosis
// ---------------------------------------------------------------------------

fn diagnose_sqlserver(cfg: &ConnectionConfig, cause: Cause, findings: &mut Vec<Finding>) {
    let (host, instance, port) = split_host(&cfg.host);
    let port = cfg.port.or(port);
    let local = is_local(&host);

    if !local {
        match cause {
            Cause::Refused | Cause::Timeout => findings.push(Finding::new(
                Level::Error,
                format!("Nothing answered at {}:{}", host, port.unwrap_or(1433)),
                "On the server: check that the SQL Server service is running, TCP/IP is enabled in SQL Server \
                 Configuration Manager, remote connections are allowed, and the firewall allows the port \
                 (1433 for the default instance).",
            )),
            Cause::LoginFailed if cfg.auth == AuthKind::Sql => findings.push(Finding::new(
                Level::Error,
                "The login was rejected — the exact reason is in the server's error log",
                "SQL Server tells the client only \"Login failed\". An administrator can run EXEC xp_readerrorlog 0, 1, N'18456' on the server; \
                 the State number explains it: 8 = wrong password, 5 = unknown login, 7 = login disabled, 18 = password must be changed, \
                 38/40 = database not available, 58 = server accepts Windows logins only.",
            )),
            Cause::InstanceNotFound => findings.push(Finding::new(
                Level::Error,
                "Named instance not found",
                "The SQL Server Browser service on the server must be running (UDP 1434 open), or connect with \
                 host,port instead of host\\instance.",
            )),
            _ => {}
        }
        return;
    }

    let Some(state) = sqlserver_state() else {
        findings.push(Finding::new(
            Level::Info,
            "Could not inspect local SQL Server",
            "PowerShell did not return the SQL Server configuration.",
        ));
        return;
    };
    if state.instances.is_empty() {
        findings.push(Finding::new(
            Level::Error,
            "No SQL Server instance is installed on this computer",
            "Install SQL Server (Developer or Express edition) or connect to another server. LocalDB is not supported because it only accepts named-pipe connections.",
        ));
        return;
    }
    let wanted = instance.clone().unwrap_or_else(|| "MSSQLSERVER".into());
    let Some(inst) = state.instances.iter().find(|i| i.name.eq_ignore_ascii_case(&wanted)) else {
        let names: Vec<String> = state.instances.iter().map(|i| i.name.clone()).collect();
        let mut f = Finding::new(
            Level::Error,
            format!("This computer has no instance named {wanted}"),
            format!("Installed instances: {}.", names.join(", ")),
        );
        if let Some(first) = state.instances.first() {
            let target = if first.name == "MSSQLSERVER" {
                "localhost".to_string()
            } else {
                format!("localhost\\{}", first.name)
            };
            f = f.with_fix(format!("Connect to {target}"), Fix::UseHost { host: target });
        }
        findings.push(f);
        return;
    };

    if !inst.status.eq_ignore_ascii_case("running") {
        findings.push(
            Finding::new(
                Level::Error,
                format!("SQL Server ({}) is not running", inst.name),
                format!("Service {} is {}.", inst.service, inst.status.to_lowercase()),
            )
            .with_fix(
                "Start the service",
                Fix::StartService {
                    service: inst.service.clone(),
                },
            ),
        );
    } else {
        findings.push(Finding::new(
            Level::Ok,
            format!("SQL Server ({}) is running", inst.name),
            "",
        ));
    }

    if inst.tcp_enabled == 0 {
        findings.push(
            Finding::new(
                Level::Error,
                "TCP/IP is disabled for this instance",
                "Sqlotus connects over TCP/IP. Enabling it restarts SQL Server, which briefly drops open connections.",
            )
            .with_fix(
                "Enable TCP/IP and restart SQL Server",
                Fix::EnableTcp {
                    instance_id: inst.id.clone(),
                    service: inst.service.clone(),
                },
            ),
        );
    } else {
        findings.push(Finding::new(Level::Ok, "TCP/IP is enabled", ""));
        let listening: Option<u16> = inst
            .port
            .split(',')
            .chain(inst.dynamic_port.split(','))
            .find_map(|p| p.trim().parse().ok())
            .filter(|p: &u16| *p > 0);
        match (instance.is_some(), port, listening) {
            // named instance without port and no browser: connect by port
            (true, None, Some(p)) if !state.browser.eq_ignore_ascii_case("running") => {
                findings.push(
                    Finding::new(
                        Level::Warning,
                        "SQL Server Browser is not running",
                        format!("Named instances are found through the SQL Server Browser. This instance listens on port {p}, so you can connect by port instead."),
                    )
                    .with_fix(format!("Use port {p}"), Fix::UsePort { port: p }),
                );
                findings.push(
                    Finding::new(
                        Level::Info,
                        "Or start the SQL Server Browser",
                        format!(
                            "Browser service is {} ({}).",
                            state.browser.to_lowercase(),
                            state.browser_start.to_lowercase()
                        ),
                    )
                    .with_fix("Enable and start SQL Server Browser", Fix::EnableBrowser),
                );
            }
            (false, None, Some(p)) if p != 1433 => findings.push(
                Finding::new(
                    Level::Warning,
                    format!("SQL Server listens on port {p}, not 1433"),
                    "The default port was changed or is dynamic.",
                )
                .with_fix(format!("Use port {p}"), Fix::UsePort { port: p }),
            ),
            // a different port is fine when something answers there (e.g. a container)
            (_, Some(p), Some(l)) if p != l && !port_open(&host, p) => findings.push(
                Finding::new(
                    Level::Warning,
                    format!("Port {p} does not match the configured port {l}"),
                    "",
                )
                .with_fix(format!("Use port {l}"), Fix::UsePort { port: l }),
            ),
            _ => {}
        }
        let fw_port = listening.or(port).unwrap_or(1433);
        match firewall_allows(fw_port) {
            Some(true) => findings.push(Finding::new(Level::Ok, format!("Windows Firewall allows TCP port {fw_port}"), "Other computers can reach this SQL Server.")),
            Some(false) => findings.push(
                Finding::new(
                    Level::Info,
                    format!("Other computers cannot reach port {fw_port}"),
                    "Windows Firewall has no inbound rule for SQL Server. This computer can still connect; others on the network cannot. The rule is added for domain and private networks only.",
                )
                .with_fix(format!("Allow port {fw_port} in Windows Firewall"), Fix::OpenFirewall { port: fw_port, browser: instance.is_some() }),
            ),
            None => {}
        }
        if matches!(cause, Cause::Refused | Cause::Timeout) && inst.status.eq_ignore_ascii_case("running") {
            let p = port.or(listening).unwrap_or(1433);
            if !port_open(&host, p) {
                findings.push(Finding::new(
                    Level::Warning,
                    format!("Port {p} is not accepting connections"),
                    "If TCP/IP was enabled recently, SQL Server must be restarted. A firewall or an IP-specific listener setting can also block it.",
                ));
            }
        }
    }

    if cause == Cause::LoginFailed && cfg.auth == AuthKind::Sql {
        if inst.login_mode == 1 {
            findings.push(
                Finding::new(
                    Level::Error,
                    "SQL Server only accepts Windows logins",
                    "The instance is in Windows Authentication mode, so SQL logins such as sa are rejected.",
                )
                .with_fix("Use Windows Authentication", Fix::UseWindowsAuth),
            );
            findings.push(
                Finding::new(Level::Info, "Or allow SQL Server logins", "Switches the instance to mixed mode and restarts it. The login (e.g. sa) must also be enabled and have a password.")
                    .with_fix("Enable SQL Server and Windows Authentication", Fix::EnableMixedAuth { instance_id: inst.id.clone(), service: inst.service.clone() }),
            );
        } else if let Some((user, reason)) = recent_login_failure(&inst.service) {
            findings.extend(login_failure_findings(&user, &reason, true));
        } else {
            findings.push(
                Finding::new(
                    Level::Error,
                    "The user name or password was rejected",
                    "Check the password (it is case-sensitive) and that the login exists and is enabled. On this computer, Windows Authentication usually works without a password.",
                )
                .with_fix("Use Windows Authentication", Fix::UseWindowsAuth),
            );
        }
    }
}

fn diagnose_service_based(cfg: &ConnectionConfig, cause: Cause, findings: &mut Vec<Finding>) {
    let (pattern, default_port, name) = match cfg.dialect {
        Dialect::Postgres => ("postgresql*", 5432, "PostgreSQL"),
        _ => ("MySQL*", 3306, "MySQL"),
    };
    let host = cfg.host.trim();
    let port = cfg.port.unwrap_or(default_port);
    if matches!(cause, Cause::Refused | Cause::Timeout) {
        if is_local(host) {
            let mut services = services_like(pattern);
            if cfg.dialect == Dialect::MySql {
                services.extend(services_like("MariaDB*"));
            }
            if services.is_empty() {
                findings.push(Finding::new(
                    Level::Warning,
                    format!("No {name} service found on this computer"),
                    format!("Nothing is listening on port {port}. If {name} runs in Docker or WSL, check that the container is started and the port is published."),
                ));
            }
            for (svc, status) in services {
                if status.eq_ignore_ascii_case("running") {
                    findings.push(Finding::new(
                        Level::Ok,
                        format!("{svc} is running"),
                        format!("Check that it listens on port {port}."),
                    ));
                } else {
                    findings.push(
                        Finding::new(
                            Level::Error,
                            format!("{svc} is not running"),
                            format!("Service status: {}.", status.to_lowercase()),
                        )
                        .with_fix("Start the service", Fix::StartService { service: svc.clone() }),
                    );
                }
            }
        } else {
            findings.push(Finding::new(
                Level::Error,
                format!("Nothing answered at {host}:{port}"),
                match cfg.dialect {
                    Dialect::Postgres => "On the server: the service must be running, listen_addresses in postgresql.conf must include this network, and the firewall must allow the port.",
                    _ => "On the server: the service must be running, bind-address must allow remote clients, and the firewall must allow the port.",
                },
            ));
        }
    }
    if cause == Cause::LoginFailed {
        findings.push(Finding::new(
            Level::Error,
            "The user name or password was rejected",
            match cfg.dialect {
                Dialect::Postgres => "Check the password. If the message mentions pg_hba.conf, the server does not allow this user/database/address combination: add a line to pg_hba.conf and reload.",
                _ => "Check the password and that the user may connect from this host (user@host in MySQL).",
            },
        ));
    }
}

pub fn diagnose(cfg: &ConnectionConfig, error: &str) -> Diagnosis {
    let cause = if error.trim().is_empty() {
        Cause::Other
    } else {
        classify(error)
    };
    let mut findings = Vec::new();

    let host_only = split_host(&cfg.host).0;
    if cause == Cause::UnknownHost || (!is_local(&host_only) && (host_only.as_str(), 1).to_socket_addrs().is_err()) {
        findings.push(Finding::new(
            Level::Error,
            format!("The server name \"{host_only}\" cannot be resolved"),
            "Check the spelling, use the IP address, or check DNS / VPN.",
        ));
    }
    match cause {
        Cause::Certificate => findings.push(
            Finding::new(
                Level::Error,
                "The server's TLS certificate is not trusted",
                "Local and development servers usually use a self-signed certificate.",
            )
            .with_fix("Trust the server certificate", Fix::TrustCertificate),
        ),
        Cause::Database => findings.push(
            Finding::new(Level::Error, "The database could not be opened", "It may not exist, be offline, or this login may have no access to it.")
                .with_fix("Connect to the default database", Fix::UseDatabase { database: None }),
        ),
        Cause::WindowsAuth => findings.push(Finding::new(
            Level::Error,
            "Windows Authentication failed",
            "Windows logins work for servers in your domain or on this computer. For other servers use SQL Server Authentication.",
        )),
        _ => {}
    }

    match cfg.dialect {
        Dialect::MsSql => diagnose_sqlserver(cfg, cause, &mut findings),
        _ => diagnose_service_based(cfg, cause, &mut findings),
    }

    let summary = match cause {
        Cause::Refused => "The server refused the connection.",
        Cause::Timeout => "The server did not answer in time.",
        Cause::UnknownHost => "The server name could not be found.",
        Cause::InstanceNotFound => "The SQL Server instance could not be located.",
        Cause::LoginFailed => "The login was rejected.",
        Cause::Certificate => "The secure connection could not be established.",
        Cause::Database => "The database could not be opened.",
        Cause::WindowsAuth => "Windows Authentication failed.",
        Cause::Other if error.trim().is_empty() => "Server check: services, network and access settings.",
        Cause::Other => "The connection failed.",
    };
    if !error.trim().is_empty() && !findings.iter().any(|f| f.level != Level::Ok) {
        findings.push(Finding::new(
            Level::Info,
            "No obvious cause found",
            "Read the original error below; it often names the exact problem.",
        ));
    }
    Diagnosis {
        summary: summary.into(),
        findings,
    }
}

// ---------------------------------------------------------------------------
// fixes that change the machine (elevated)
// ---------------------------------------------------------------------------

fn ps_quote(s: &str) -> String {
    format!("'{}'", s.replace('\'', "''"))
}

fn fix_script(fix: &Fix) -> Option<String> {
    let base = |id: &str| format!("HKLM:\\SOFTWARE\\Microsoft\\Microsoft SQL Server\\{id}\\MSSQLServer");
    Some(match fix {
        Fix::EnableTcp { instance_id, service } => format!(
            "Set-ItemProperty -Path ({}) -Name Enabled -Value 1\nRestart-Service -Name {} -Force",
            ps_quote(&format!("{}\\SuperSocketNetLib\\Tcp", base(instance_id))),
            ps_quote(service)
        ),
        Fix::StartService { service } => format!("Start-Service -Name {}", ps_quote(service)),
        Fix::EnableBrowser => {
            "Set-Service -Name SQLBrowser -StartupType Automatic\nStart-Service -Name SQLBrowser".into()
        }
        Fix::EnableMixedAuth { instance_id, service } => format!(
            "Set-ItemProperty -Path {} -Name LoginMode -Value 2\nRestart-Service -Name {} -Force",
            ps_quote(&base(instance_id)),
            ps_quote(service)
        ),
        Fix::OpenFirewall { port, browser } => {
            let mut s = format!(
                "New-NetFirewallRule -DisplayName 'Sqlotus: SQL Server (TCP {port})' -Direction Inbound -Protocol TCP -LocalPort {port} -Action Allow -Profile Domain,Private | Out-Null"
            );
            if *browser {
                s.push_str("\nNew-NetFirewallRule -DisplayName 'Sqlotus: SQL Server Browser (UDP 1434)' -Direction Inbound -Protocol UDP -LocalPort 1434 -Action Allow -Profile Domain,Private | Out-Null");
            }
            s
        }
        _ => return None,
    })
}

/// Run an administrative fix through a UAC prompt. Returns a message for
/// the user either way.
fn run_elevated(body: &str) -> Result<String, String> {
    let dir = std::env::temp_dir().join(format!("sqlotus-fix-{}", std::process::id()));
    std::fs::create_dir_all(&dir).map_err(|e| e.to_string())?;
    let script = dir.join("fix.ps1");
    let log = dir.join("fix.log");
    let _ = std::fs::remove_file(&log);
    let wrapped = format!(
        "$ErrorActionPreference = 'Stop'\ntry {{\n{body}\n'OK' | Out-File -FilePath {log} -Encoding utf8\n}} catch {{\n\"ERROR: $_\" | Out-File -FilePath {log} -Encoding utf8\n}}\n",
        log = ps_quote(&log.to_string_lossy())
    );
    std::fs::write(&script, wrapped).map_err(|e| e.to_string())?;
    let launcher = format!(
        "try {{ Start-Process powershell -Verb RunAs -Wait -WindowStyle Hidden -ArgumentList '-NoProfile','-ExecutionPolicy','Bypass','-File',{} ; 'launched' }} catch {{ 'declined' }}",
        ps_quote(&format!("\"{}\"", script.to_string_lossy()))
    );
    let launched = powershell(&launcher).unwrap_or_default();
    let result = std::fs::read_to_string(&log).unwrap_or_default();
    let _ = std::fs::remove_dir_all(&dir);
    if launched.contains("declined") {
        return Err("Administrator permission was not granted, so nothing was changed.".into());
    }
    let result = result.trim_start_matches('\u{feff}').trim();
    if result.starts_with("OK") {
        Ok("Done.".into())
    } else if let Some(e) = result.strip_prefix("ERROR:") {
        Err(e.trim().to_string())
    } else {
        Err("The fix did not report a result.".into())
    }
}

/// Enable a SQL login through a Windows Authentication connection.
async fn enable_login(config: &ConnectionConfig, login: &str) -> Result<String, String> {
    let admin = ConnectionConfig {
        auth: AuthKind::Windows,
        database: None,
        user: None,
        ..config.clone()
    };
    let mut conn = Connection::open(&admin, None)
        .await
        .map_err(|e| format!("Could not connect with your Windows login: {e}"))?;
    let sql = format!("ALTER LOGIN [{}] ENABLE", login.replace(']', "]]"));
    let result = conn.query_strings(&sql).await;
    conn.close().await;
    result.map_err(|e| format!("Could not enable the login (sysadmin rights are needed): {e}"))?;
    Ok(format!("Login {login} enabled."))
}

#[tauri::command]
pub async fn diagnose_connection(config: ConnectionConfig, error: String) -> CmdResult<Diagnosis> {
    tauri::async_runtime::spawn_blocking(move || diagnose(&config, &error))
        .await
        .map_err(|e| e.to_string())
}

/// Apply a machine-level fix (elevated), then try the connection again.
#[tauri::command]
pub async fn apply_fix(
    state: tauri::State<'_, crate::state::AppState>,
    fix: Fix,
    config: ConnectionConfig,
    password: Option<String>,
) -> CmdResult<String> {
    let password = password
        .filter(|p| !p.is_empty())
        .or_else(|| state.store.get_password(&config.id).ok().flatten());
    let message = if let Fix::EnableLogin { login } = &fix {
        enable_login(&config, login).await?
    } else {
        let Some(body) = fix_script(&fix) else {
            return Err("This fix changes the connection settings and is applied in the dialog.".into());
        };
        tauri::async_runtime::spawn_blocking(move || run_elevated(&body))
            .await
            .map_err(|e| e.to_string())??
    };
    // give a restarted service a moment, then verify
    for attempt in 0..6 {
        if attempt > 0 {
            tokio::time::sleep(Duration::from_secs(2)).await;
        }
        if let Ok(conn) = Connection::open(&config, password.as_deref()).await {
            conn.close().await;
            return Ok(format!("{message} The connection works now."));
        }
    }
    Ok(format!(
        "{message} The connection still fails — run the diagnosis again."
    ))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn classifies_driver_errors() {
        assert_eq!(
            classify(
                "connection failed: An error occurred during the attempt of performing I/O: No connection could be made because the target machine actively refused it. (os error 10061)"
            ),
            Cause::Refused
        );
        assert_eq!(
            classify("Msg 18456, Level 14, State 1, Line 1\nLogin failed for user 'sa'."),
            Cause::LoginFailed
        );
        assert_eq!(
            classify("Cannot open database \"Nope\" requested by the login. (4060)"),
            Cause::Database
        );
        assert_eq!(
            classify("FATAL: password authentication failed for user \"postgres\""),
            Cause::LoginFailed
        );
        assert_eq!(
            classify("db error: FATAL: database \"x\" does not exist"),
            Cause::Database
        );
        assert_eq!(
            classify("Access denied for user 'root'@'172.17.0.1' (using password: YES)"),
            Cause::LoginFailed
        );
        assert_eq!(
            classify("certificate verify failed: self-signed certificate"),
            Cause::Certificate
        );
        assert_eq!(classify("timed out after 20s"), Cause::Timeout);
        assert_eq!(classify("No such host is known. (os error 11001)"), Cause::UnknownHost);
    }

    #[test]
    fn host_parsing_and_locality() {
        assert_eq!(
            split_host(r".\SQLEXPRESS,1500"),
            (".".into(), Some("SQLEXPRESS".into()), Some(1500))
        );
        assert!(is_local("localhost") && is_local(".") && is_local("127.0.0.1"));
        assert!(!is_local("db.example.com"));
    }

    #[test]
    fn fixes_and_scripts() {
        let f = Fix::EnableTcp {
            instance_id: "MSSQL16.MSSQLSERVER".into(),
            service: "MSSQLSERVER".into(),
        };
        assert!(f.needs_admin());
        let s = fix_script(&f).unwrap();
        assert!(
            s.contains(r"MSSQL16.MSSQLSERVER\MSSQLServer\SuperSocketNetLib\Tcp")
                && s.contains("Restart-Service -Name 'MSSQLSERVER'")
        );
        assert!(fix_script(&Fix::TrustCertificate).is_none());
        let json = serde_json::to_string(&Fix::UsePort { port: 1500 }).unwrap();
        assert_eq!(json, r#"{"kind":"usePort","port":1500}"#);
    }

    #[test]
    fn login_failure_reasons() {
        let (user, reason) = parse_login_failure(
            "Login failed for user 'sa'. Reason: Password did not match that for the login provided. [CLIENT: <local machine>]",
        )
        .unwrap();
        assert_eq!(
            (user.as_str(), reason.as_str()),
            ("sa", "Password did not match that for the login provided")
        );
        let f = login_failure_findings("sa", "The account is disabled", true);
        assert_eq!(f[0].fix, Some(Fix::EnableLogin { login: "sa".into() }));
        assert!(!f[0].needs_admin);
        let f = login_failure_findings("x", "Could not find a login matching the name provided", false);
        assert!(f[0].title.contains("does not exist") && f[0].fix.is_none());
        let s = fix_script(&Fix::OpenFirewall {
            port: 1433,
            browser: true,
        })
        .unwrap();
        assert!(s.contains("-LocalPort 1433") && s.contains("UDP") && s.contains("Domain,Private"));
    }

    #[test]
    fn remote_refused_gives_server_side_advice() {
        let cfg = ConnectionConfig {
            dialect: Dialect::MsSql,
            host: "10.255.255.1".into(),
            ..Default::default()
        };
        let d = diagnose(&cfg, "actively refused (os error 10061)");
        assert!(
            d.findings
                .iter()
                .any(|f| f.title.starts_with("Nothing answered at 10.255.255.1:1433")),
            "{d:?}"
        );
    }
}
