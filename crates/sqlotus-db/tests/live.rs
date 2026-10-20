//! Live tests against real servers. Each test is skipped unless its
//! environment variable holds a JSON `ConnectionConfig` (plus optional
//! `password` field), e.g.
//!
//! SQLOTUS_TEST_MSSQL='{"dialect":"mssql","host":"localhost","user":"sa","password":"...","database":"Shop"}'
//! SQLOTUS_TEST_PG='{"dialect":"postgres","host":"localhost","user":"postgres","password":"...","database":"shop"}'
//! SQLOTUS_TEST_MYSQL='{"dialect":"mysql","host":"localhost","user":"root","password":"...","database":"shop"}'
//!
//! The deeper checks expect the sample schema from `scripts/test-db/` (tables
//! customers/orders with a foreign key, a view, a procedure, a function).

use std::time::{Duration, Instant};

use serde::Deserialize;
use sqlotus_core::catalog::ObjectKind;
use sqlotus_core::search::{MatchIn, SearchOptions, search};
use sqlotus_core::{CompletionKind, CompletionOptions, complete};
use sqlotus_db::{
    BatchOutput, Connection, ConnectionConfig, Limits, MessageKind, execute_script, load_catalog,
};

#[derive(Deserialize)]
struct Env {
    #[serde(flatten)]
    config: ConnectionConfig,
    password: Option<String>,
}

fn env(var: &str) -> Option<Env> {
    let raw = std::env::var(var).ok()?;
    Some(serde_json::from_str(&raw).expect("valid JSON config"))
}

async fn connect(e: &Env) -> Connection {
    Connection::open(&e.config, e.password.as_deref())
        .await
        .expect("connect")
}

fn errors(out: &[sqlotus_db::ScriptBatch]) -> Vec<String> {
    out.iter()
        .flat_map(|b| &b.output.messages)
        .filter(|m| m.kind == MessageKind::Error)
        .map(|m| m.text.clone())
        .collect()
}

fn first_error(out: &BatchOutput) -> String {
    out.messages
        .iter()
        .find(|m| m.kind == MessageKind::Error)
        .map(|m| m.text.clone())
        .unwrap_or_default()
}

/// Shared checks: script execution, catalog contents, completion and search
/// against the real catalog, error reporting.
async fn exercise(var: &str, script: &str, expected_sets: usize, names: Names) {
    let Some(e) = env(var) else {
        eprintln!("{var} not set, skipping");
        return;
    };
    let dialect = e.config.dialect;
    let mut conn = connect(&e).await;

    // scripts, batches, multiple result sets
    let out = execute_script(&mut conn, script, &Limits::default()).await;
    assert!(errors(&out).is_empty(), "{:?}", errors(&out));
    let sets: Vec<_> = out.iter().flat_map(|b| &b.output.result_sets).collect();
    assert_eq!(sets.len(), expected_sets, "{out:#?}");
    for rs in &sets {
        assert!(!rs.columns.is_empty());
    }

    // unicode round trip
    let rows = conn
        .query_strings(&format!(
            "SELECT name FROM {} WHERE id = 1",
            names.customers
        ))
        .await
        .expect("query");
    assert_eq!(rows[0][0].as_deref(), Some("Nguyễn Văn A"));

    // errors come back as messages, and the connection stays usable
    let bad = conn
        .execute(
            "SELECT * FROM table_that_does_not_exist",
            &Limits::default(),
        )
        .await;
    assert!(bad.failed, "{bad:?}");
    let err = first_error(&bad);
    assert!(
        err.to_lowercase().contains("table_that_does_not_exist"),
        "{err}"
    );
    assert!(conn.query_strings("SELECT 1").await.is_ok());

    // row limit
    let limited = conn
        .execute(
            &format!("SELECT * FROM {}", names.customers),
            &Limits {
                max_rows: 1,
                max_cell_chars: 10,
            },
        )
        .await;
    assert!(limited.result_sets[0].truncated);
    assert_eq!(limited.result_sets[0].rows.len(), 1);

    // catalog
    let snap = load_catalog(&mut conn).await.expect("catalog");
    let cat = &snap.catalog;
    let orders = cat
        .find(None, names.orders_table)
        .expect("orders in catalog");
    assert_eq!(orders.kind, ObjectKind::Table);
    assert!(
        orders
            .columns
            .iter()
            .any(|c| c.name.eq_ignore_ascii_case(names.fk_column)),
        "{orders:?}"
    );
    assert!(
        orders.columns.iter().any(|c| c.is_primary_key),
        "{orders:?}"
    );
    assert!(
        cat.objects.iter().any(|o| o.kind == ObjectKind::View),
        "no view"
    );
    let proc = cat
        .objects
        .iter()
        .find(|o| o.kind == ObjectKind::Procedure)
        .expect("procedure");
    assert!(!proc.params.is_empty(), "{proc:?}");
    assert!(
        cat.foreign_keys
            .iter()
            .any(|fk| fk.from_table.eq_ignore_ascii_case(names.orders_table)
                && fk.to_table.eq_ignore_ascii_case(names.customers_table)),
        "{:?}",
        cat.foreign_keys
    );
    assert!(
        snap.definitions.iter().any(|d| d.kind == ObjectKind::View),
        "no view definition"
    );
    assert!(
        snap.definitions
            .iter()
            .any(|d| d.kind == ObjectKind::Procedure),
        "no proc definition"
    );

    // completion on the real catalog: FK join after JOIN
    let src = format!("SELECT * FROM {} o JOIN ", names.orders);
    let r = complete(
        &src,
        src.len(),
        dialect,
        Some(cat),
        &[],
        &CompletionOptions::default(),
    );
    let join = r
        .items
        .iter()
        .find(|i| i.kind == CompletionKind::Join)
        .expect("join suggestion");
    assert!(join.insert_text.contains(" ON "), "{join:?}");
    // columns after alias dot
    let src = format!("SELECT o. FROM {} o", names.orders);
    let r = complete(
        &src,
        9,
        dialect,
        Some(cat),
        &[],
        &CompletionOptions::default(),
    );
    assert!(
        r.items
            .iter()
            .any(|i| i.label.eq_ignore_ascii_case(names.fk_column)),
        "{:?}",
        r.items
    );

    // SQL Search: column hit and text hit inside the procedure/view definitions
    let hits = search(
        names.fk_column,
        &cat.database,
        cat,
        &snap.definitions,
        &SearchOptions::default(),
    );
    assert!(hits.iter().any(|h| h.matched_in == MatchIn::Column));
    assert!(
        hits.iter().any(|h| h.matched_in == MatchIn::Definition),
        "{hits:?}"
    );

    let dbs = conn.list_databases().await.expect("databases");
    assert!(
        dbs.iter().any(|d| d.eq_ignore_ascii_case(&cat.database)),
        "{dbs:?}"
    );
    conn.close().await;
}

struct Names {
    customers: &'static str,
    orders: &'static str,
    customers_table: &'static str,
    orders_table: &'static str,
    fk_column: &'static str,
}

#[tokio::test]
async fn mssql() {
    exercise(
        "SQLOTUS_TEST_MSSQL",
        "SELECT 1 AS a, N'ä' AS b, CAST(1.5 AS decimal(5,2)) AS c, GETDATE() AS d, NEWID() AS e, CAST(NULL AS int) AS f, \
         CAST(0x0A0B AS varbinary(2)) AS g, CAST('2024-01-02T03:04:05.1234567' AS datetime2) AS h, CAST(1 AS bit) AS i\n\
         GO\nDECLARE @t TABLE (id int); INSERT INTO @t VALUES (1),(2); SELECT * FROM @t\nGO 2\n\
         EXEC dbo.GetCustomerOrders @CustomerId = 1",
        4,
        Names {
            customers: "dbo.Customers",
            orders: "dbo.Orders",
            customers_table: "Customers",
            orders_table: "Orders",
            fk_column: "CustomerId",
        },
    )
    .await;
}

#[tokio::test]
async fn mssql_value_rendering_and_use() {
    let Some(e) = env("SQLOTUS_TEST_MSSQL") else {
        return;
    };
    let mut conn = connect(&e).await;
    let out = conn
        .execute(
            "SELECT CAST(0x0A0B AS varbinary(2)) AS g, CAST('2024-01-02T03:04:05.1234567' AS datetime2) AS h, \
             CAST(1 AS bit) AS i, CAST('2024-01-02 03:04:05.123' AS datetime) AS j, CAST(12.5 AS money) AS k",
            &Limits::default(),
        )
        .await;
    let row = &out.result_sets[0].rows[0];
    assert_eq!(row[0].as_deref(), Some("0x0A0B"));
    assert_eq!(row[1].as_deref(), Some("2024-01-02 03:04:05.1234567"));
    assert_eq!(row[2].as_deref(), Some("1"));
    assert_eq!(row[3].as_deref(), Some("2024-01-02 03:04:05.123"));
    assert_eq!(row[4].as_deref(), Some("12.5"));
    conn.use_database("master").await.expect("use");
    assert_eq!(conn.current_database().await.unwrap(), "master");
    // server error formatting like SSMS
    let bad = conn.execute("SELECT 1/0", &Limits::default()).await;
    assert!(first_error(&bad).starts_with("Msg 8134"), "{bad:?}");
}

#[tokio::test]
async fn mssql_cancel_by_abort() {
    let Some(e) = env("SQLOTUS_TEST_MSSQL") else {
        return;
    };
    let conn = connect(&e).await;
    let started = Instant::now();
    let task = tokio::spawn(async move {
        let mut conn = conn;
        conn.execute("WAITFOR DELAY '00:00:30'", &Limits::default())
            .await
    });
    tokio::time::sleep(Duration::from_millis(500)).await;
    task.abort();
    assert!(task.await.unwrap_err().is_cancelled());
    assert!(started.elapsed() < Duration::from_secs(5));
}

#[tokio::test]
async fn postgres() {
    exercise(
        "SQLOTUS_TEST_PG",
        "SELECT 1 AS a, 'x'::text AS b, now() AS c; CREATE TEMP TABLE t(id int); INSERT INTO t VALUES (1); SELECT * FROM t; \
         SELECT order_count(1);",
        3,
        Names { customers: "customers", orders: "orders", customers_table: "customers", orders_table: "orders", fk_column: "customer_id" },
    )
    .await;
}

#[tokio::test]
async fn postgres_notice_and_cancel() {
    let Some(e) = env("SQLOTUS_TEST_PG") else {
        return;
    };
    let mut conn = connect(&e).await;
    let out = conn
        .execute("CALL touch_customer(1)", &Limits::default())
        .await;
    assert!(!out.failed, "{out:?}");
    // the notice arrives on the connection task; give it a moment, then the next batch reports it
    tokio::time::sleep(Duration::from_millis(200)).await;
    let next = conn.execute("SELECT 1", &Limits::default()).await;
    let texts: Vec<_> = out
        .messages
        .iter()
        .chain(&next.messages)
        .map(|m| m.text.clone())
        .collect();
    assert!(texts.iter().any(|t| t.contains("touching 1")), "{texts:?}");

    let cancel = conn.cancel_handle();
    let started = Instant::now();
    let canceller = tokio::spawn(async move {
        tokio::time::sleep(Duration::from_millis(500)).await;
        cancel.cancel().await.expect("cancel");
    });
    let out = conn
        .execute("SELECT pg_sleep(30)", &Limits::default())
        .await;
    canceller.await.unwrap();
    assert!(
        out.failed && first_error(&out).contains("cancel"),
        "{out:?}"
    );
    assert!(started.elapsed() < Duration::from_secs(5));
    assert!(conn.query_strings("SELECT 1").await.is_ok());
}

#[tokio::test]
async fn mysql() {
    exercise(
        "SQLOTUS_TEST_MYSQL",
        "SELECT 1 AS a, NOW() AS b; CREATE TEMPORARY TABLE t(id int); INSERT INTO t VALUES (1); SELECT * FROM t; \
         CALL get_customer_orders(1);",
        3,
        Names { customers: "customers", orders: "orders", customers_table: "customers", orders_table: "orders", fk_column: "customer_id" },
    )
    .await;
}

#[tokio::test]
async fn mysql_cancel() {
    let Some(e) = env("SQLOTUS_TEST_MYSQL") else {
        return;
    };
    let mut conn = connect(&e).await;
    let cancel = conn.cancel_handle();
    let started = Instant::now();
    let canceller = tokio::spawn(async move {
        tokio::time::sleep(Duration::from_millis(500)).await;
        cancel.cancel().await.expect("cancel");
    });
    let out = conn.execute("SELECT SLEEP(30)", &Limits::default()).await;
    canceller.await.unwrap();
    // KILL QUERY makes SLEEP return 1 early (or errors, depending on version)
    assert!(started.elapsed() < Duration::from_secs(5), "{out:?}");
    assert!(conn.query_strings("SELECT 1").await.is_ok());
}

/// Row counts inside a transaction, estimated plans and variable values.
async fn safety_features(var: &str, customers: &str) {
    let Some(e) = env(var) else {
        return;
    };
    let dialect = e.config.dialect;
    let mut conn = connect(&e).await;
    let limits = Limits::default();

    // a counted change inside a transaction, then rolled back
    let begin = conn.begin_statement();
    assert!(!conn.execute(begin, &limits).await.failed);
    let options = sqlotus_db::ScriptOptions { counted: true, ..Default::default() };
    let out = sqlotus_db::execute_script_with(
        &mut conn,
        &format!("UPDATE {customers} SET name = name WHERE id IN (1, 2)"),
        &limits,
        &options,
    )
    .await;
    assert!(errors(&out).is_empty(), "{:?}", errors(&out));
    let changed: u64 = out.iter().map(|b| b.output.rows_affected).sum();
    assert_eq!(changed, 2, "{out:#?}");
    assert_eq!(conn.open_transactions().await.unwrap(), 1);
    assert!(!conn.execute("ROLLBACK", &limits).await.failed);
    assert_eq!(conn.open_transactions().await.unwrap(), 0);

    // estimated plan
    let plan = conn.explain(&format!("SELECT name FROM {customers} WHERE name LIKE 'A%'")).await;
    assert!(plan.errors.is_empty(), "{:?}", plan.errors);
    assert_eq!(plan.plans.len(), 1, "{plan:#?}");
    assert!(plan.plans[0].root.is_some());
    // nothing ran and the session is back to normal
    assert_eq!(conn.query_strings("SELECT 1").await.unwrap()[0][0].as_deref(), Some("1"));

    // values for undeclared variables
    if dialect != sqlotus_core::Dialect::Postgres {
        let options = sqlotus_db::ScriptOptions {
            variables: vec![sqlotus_core::variables::VariableValue {
                name: "@x".into(),
                data_type: "int".into(),
                value: Some("5".into()),
            }],
            ..Default::default()
        };
        let out = sqlotus_db::execute_script_with(&mut conn, "SELECT @x + 1 AS v", &limits, &options).await;
        assert!(errors(&out).is_empty(), "{:?}", errors(&out));
        assert_eq!(out[0].output.result_sets.last().unwrap().rows[0][0].as_deref(), Some("6"));
    }
}

#[tokio::test]
async fn mssql_safety() {
    safety_features("SQLOTUS_TEST_MSSQL", "dbo.Customers").await;
}

#[tokio::test]
async fn postgres_safety() {
    safety_features("SQLOTUS_TEST_PG", "customers").await;
}

#[tokio::test]
async fn mysql_safety() {
    safety_features("SQLOTUS_TEST_MYSQL", "customers").await;
}
