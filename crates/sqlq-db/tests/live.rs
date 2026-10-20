//! Live tests against real servers. Each test is skipped unless its
//! environment variable holds a JSON `ConnectionConfig` (plus optional
//! `password` field), e.g.
//!
//! SQLQ_TEST_MSSQL='{"dialect":"mssql","host":"localhost","user":"sa","password":"...","trustServerCertificate":true}'
//! SQLQ_TEST_PG='{"dialect":"postgres","host":"localhost","user":"postgres","password":"..."}'
//! SQLQ_TEST_MYSQL='{"dialect":"mysql","host":"localhost","user":"root","password":"...","database":"mysql"}'

use serde::Deserialize;
use sqlq_db::{Connection, ConnectionConfig, Limits, MessageKind, execute_script, load_catalog};

#[derive(Deserialize)]
struct Env {
    #[serde(flatten)]
    config: ConnectionConfig,
    password: Option<String>,
}

async fn connect(var: &str) -> Option<Connection> {
    let raw = std::env::var(var).ok()?;
    let env: Env = serde_json::from_str(&raw).expect("valid JSON config");
    Some(
        Connection::open(&env.config, env.password.as_deref())
            .await
            .expect("connect"),
    )
}

async fn roundtrip(var: &str, script: &str, expected_sets: usize) {
    let Some(mut conn) = connect(var).await else {
        eprintln!("{var} not set, skipping");
        return;
    };
    let out = execute_script(&mut conn, script, &Limits::default()).await;
    let errors: Vec<_> = out
        .iter()
        .flat_map(|b| &b.output.messages)
        .filter(|m| m.kind == MessageKind::Error)
        .collect();
    assert!(errors.is_empty(), "{errors:?}");
    let sets: usize = out.iter().map(|b| b.output.result_sets.len()).sum();
    assert_eq!(sets, expected_sets);

    let snap = load_catalog(&mut conn).await.expect("catalog");
    assert!(!snap.catalog.database.is_empty());
    let dbs = conn.list_databases().await.expect("databases");
    assert!(!dbs.is_empty());
}

#[tokio::test]
async fn mssql() {
    roundtrip(
        "SQLQ_TEST_MSSQL",
        "SELECT 1 AS a, N'ä' AS b, CAST(1.5 AS decimal(5,2)) AS c, GETDATE() AS d, NEWID() AS e, CAST(NULL AS int) AS f\n\
         GO\nCREATE TABLE #t (id int); INSERT INTO #t VALUES (1),(2); SELECT * FROM #t\nGO 2",
        3,
    )
    .await;
}

#[tokio::test]
async fn postgres() {
    roundtrip("SQLQ_TEST_PG", "SELECT 1 AS a, 'x'::text AS b, now() AS c; CREATE TEMP TABLE t(id int); INSERT INTO t VALUES (1); SELECT * FROM t;", 2).await;
}

#[tokio::test]
async fn mysql() {
    roundtrip("SQLQ_TEST_MYSQL", "SELECT 1 AS a, NOW() AS b; CREATE TEMPORARY TABLE t(id int); INSERT INTO t VALUES (1); SELECT * FROM t;", 2).await;
}
