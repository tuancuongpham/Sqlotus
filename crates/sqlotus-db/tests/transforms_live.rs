//! The editor's rewrites checked by real servers (skipped unless the
//! SQLOTUS_TEST_* variables are set, see `live.rs`): every case of the corpus
//! in `sqlotus-core/tests/corpus` runs as written and after each rewrite —
//! formatting in three styles, keyword case, inserted semicolons, removed
//! brackets. A rewrite must never turn valid SQL into a syntax error.
//!
//! SQL Server only parses (`SET PARSEONLY ON`). PostgreSQL runs each
//! statement in a transaction that is rolled back; MySQL runs the scripts in
//! a scratch database. Only syntax errors count: the corpus names tables
//! that do not exist.

use serde::Deserialize;
use sqlotus_core::format::{CommaStyle, KeywordCase, ListLayout};
use sqlotus_core::statements::split_statements;
use sqlotus_core::{Dialect, FormatOptions, format_sql, refactor};
use sqlotus_db::{Connection, ConnectionConfig, Limits, MessageKind, execute_script};

#[derive(Deserialize)]
struct Env {
    #[serde(flatten)]
    config: ConnectionConfig,
    password: Option<String>,
}

fn env(var: &str) -> Option<Env> {
    Some(serde_json::from_str(&std::env::var(var).ok()?).expect("valid JSON config"))
}

async fn connect(e: &Env, database: Option<&str>) -> Connection {
    let mut cfg = e.config.clone();
    if let Some(db) = database {
        cfg.database = Some(db.to_string());
    }
    Connection::open(&cfg, e.password.as_deref()).await.expect("connect")
}

fn cases(dialect: Dialect) -> Vec<(String, String)> {
    let text = match dialect {
        Dialect::MsSql => include_str!("../../sqlotus-core/tests/corpus/mssql.sql"),
        Dialect::Postgres => include_str!("../../sqlotus-core/tests/corpus/postgres.sql"),
        Dialect::MySql => include_str!("../../sqlotus-core/tests/corpus/mysql.sql"),
    };
    text.replace("\r\n", "\n")
        .split("-- @case ")
        .skip(1)
        .map(|part| {
            let (name, body) = part.split_once('\n').expect("a case name line");
            (name.trim().to_string(), format!("{}\n", body.trim_end()))
        })
        .collect()
}

fn variants(src: &str, d: Dialect) -> Vec<(&'static str, String)> {
    let base = FormatOptions::default();
    let stacked = FormatOptions {
        keyword_case: KeywordCase::Lower,
        comma_style: CommaStyle::Leading,
        list_layout: ListLayout::Stacked,
        ..base.clone()
    };
    let compact = FormatOptions {
        indent_size: 2,
        indent_joins: false,
        on_new_line: false,
        and_or_new_line: false,
        blank_line_between_statements: false,
        ..base.clone()
    };
    let formatted = format_sql(src, d, &base);
    vec![
        ("formatted", formatted.clone()),
        ("formatted lower, leading commas, stacked", format_sql(src, d, &stacked)),
        ("formatted compact", format_sql(src, d, &compact)),
        (
            "keywords upper",
            refactor::apply_keyword_case(src, d, KeywordCase::Upper),
        ),
        (
            "keywords lower",
            refactor::apply_keyword_case(src, d, KeywordCase::Lower),
        ),
        ("semicolons", refactor::insert_semicolons(src, d)),
        ("formatted, then semicolons", refactor::insert_semicolons(&formatted, d)),
        ("no brackets", refactor::remove_brackets(src, d)),
    ]
}

fn is_syntax_error(d: Dialect, text: &str) -> bool {
    match d {
        // with PARSEONLY every error is a parse error
        Dialect::MsSql => true,
        Dialect::Postgres => text.contains("syntax error") || text.contains("unterminated"),
        Dialect::MySql => text.contains("error in your SQL syntax"),
    }
}

fn errors(d: Dialect, messages: impl IntoIterator<Item = sqlotus_db::Message>) -> Vec<String> {
    messages
        .into_iter()
        .filter(|m| m.kind == MessageKind::Error && is_syntax_error(d, &m.text))
        .map(|m| m.text)
        .collect()
}

async fn syntax_errors(conn: &mut Connection, d: Dialect, script: &str) -> Vec<String> {
    let limits = Limits::default();
    match d {
        Dialect::MsSql | Dialect::MySql => {
            let out = execute_script(conn, script, &limits).await;
            errors(d, out.into_iter().flat_map(|b| b.output.messages))
        }
        Dialect::Postgres => {
            // one statement at a time, each behind a savepoint, all rolled back
            let mut found = Vec::new();
            conn.execute("BEGIN", &limits).await;
            for span in split_statements(script, d) {
                let stmt = span.text(script);
                let first = stmt
                    .split(|c: char| !c.is_alphanumeric())
                    .find(|w| !w.is_empty())
                    .unwrap_or_default()
                    .to_ascii_uppercase();
                if matches!(
                    first.as_str(),
                    "BEGIN" | "COMMIT" | "ROLLBACK" | "SAVEPOINT" | "RELEASE" | "END" | "START"
                ) {
                    continue;
                }
                conn.execute("SAVEPOINT t", &limits).await;
                found.extend(errors(d, conn.execute(stmt, &limits).await.messages));
                conn.execute("ROLLBACK TO SAVEPOINT t", &limits).await;
            }
            conn.execute("ROLLBACK", &limits).await;
            found
        }
    }
}

async fn check(var: &str) {
    let Some(e) = env(var) else {
        eprintln!("{var} not set, skipping");
        return;
    };
    let d = e.config.dialect;
    let limits = Limits::default();
    let scratch = "sqlotus_syntax";
    let mut conn = match d {
        Dialect::MsSql => {
            let mut c = connect(&e, None).await;
            assert!(!c.execute("SET PARSEONLY ON", &limits).await.failed);
            c
        }
        Dialect::Postgres => {
            let mut admin = connect(&e, None).await;
            admin
                .execute(&format!("DROP DATABASE IF EXISTS {scratch} WITH (FORCE)"), &limits)
                .await;
            assert!(
                !admin
                    .execute(&format!("CREATE DATABASE {scratch}"), &limits)
                    .await
                    .failed
            );
            admin.close().await;
            connect(&e, Some(scratch)).await
        }
        Dialect::MySql => {
            let mut admin = connect(&e, None).await;
            let out = admin
                .execute(
                    &format!("DROP DATABASE IF EXISTS {scratch}; CREATE DATABASE {scratch}"),
                    &limits,
                )
                .await;
            assert!(!out.failed, "{:?}", out.messages);
            admin.close().await;
            connect(&e, Some(scratch)).await
        }
    };

    let mut problems = Vec::new();
    let mut checked = 0;
    for (name, src) in cases(d) {
        let base = syntax_errors(&mut conn, d, &src).await;
        if !base.is_empty() {
            problems.push(format!("{name}: syntax errors as written (fix the corpus): {base:?}"));
            continue;
        }
        for (label, text) in variants(&src, d) {
            if text == src {
                continue;
            }
            checked += 1;
            let found = syntax_errors(&mut conn, d, &text).await;
            if !found.is_empty() {
                problems.push(format!("{name} [{label}]: {found:?}\n--- text ---\n{text}"));
            }
        }
    }
    if d == Dialect::MsSql {
        conn.execute("SET PARSEONLY OFF", &limits).await;
    }
    conn.close().await;
    if d != Dialect::MsSql {
        let mut admin = connect(&e, None).await;
        let drop = match d {
            Dialect::Postgres => format!("DROP DATABASE IF EXISTS {scratch} WITH (FORCE)"),
            _ => format!("DROP DATABASE IF EXISTS {scratch}"),
        };
        admin.execute(&drop, &limits).await;
        admin.close().await;
    }
    eprintln!("{var}: {checked} rewritten scripts checked");
    assert!(
        problems.is_empty(),
        "{} problem(s):\n\n{}",
        problems.len(),
        problems.join("\n\n")
    );
}

#[tokio::test]
async fn mssql_rewrites_keep_valid_syntax() {
    check("SQLOTUS_TEST_MSSQL").await;
}

#[tokio::test]
async fn postgres_rewrites_keep_valid_syntax() {
    check("SQLOTUS_TEST_PG").await;
}

#[tokio::test]
async fn mysql_rewrites_keep_valid_syntax() {
    check("SQLOTUS_TEST_MYSQL").await;
}
