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
use sqlotus_db::{BatchOutput, Connection, ConnectionConfig, Limits, MessageKind, execute_script, load_catalog};

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
        .query_strings(&format!("SELECT name FROM {} WHERE id = 1", names.customers))
        .await
        .expect("query");
    assert_eq!(rows[0][0].as_deref(), Some("Nguyễn Văn A"));

    // errors come back as messages, and the connection stays usable
    let bad = conn
        .execute("SELECT * FROM table_that_does_not_exist", &Limits::default())
        .await;
    assert!(bad.failed, "{bad:?}");
    let err = first_error(&bad);
    assert!(err.to_lowercase().contains("table_that_does_not_exist"), "{err}");
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
    let orders = cat.find(None, names.orders_table).expect("orders in catalog");
    assert_eq!(orders.kind, ObjectKind::Table);
    assert!(
        orders
            .columns
            .iter()
            .any(|c| c.name.eq_ignore_ascii_case(names.fk_column)),
        "{orders:?}"
    );
    assert!(orders.columns.iter().any(|c| c.is_primary_key), "{orders:?}");
    assert!(cat.objects.iter().any(|o| o.kind == ObjectKind::View), "no view");
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
        snap.definitions.iter().any(|d| d.kind == ObjectKind::Procedure),
        "no proc definition"
    );

    // completion on the real catalog: FK join after JOIN
    let src = format!("SELECT * FROM {} o JOIN ", names.orders);
    let r = complete(&src, src.len(), dialect, Some(cat), &[], &CompletionOptions::default());
    let join = r
        .items
        .iter()
        .find(|i| i.kind == CompletionKind::Join)
        .expect("join suggestion");
    assert!(join.insert_text.contains(" ON "), "{join:?}");
    // columns after alias dot
    let src = format!("SELECT o. FROM {} o", names.orders);
    let r = complete(&src, 9, dialect, Some(cat), &[], &CompletionOptions::default());
    assert!(
        r.items.iter().any(|i| i.label.eq_ignore_ascii_case(names.fk_column)),
        "{:?}",
        r.items
    );

    // Object Search: column hit and text hit inside the procedure/view definitions
    let hits = search(
        names.fk_column,
        &cat.database,
        cat,
        &snap.definitions,
        &SearchOptions::default(),
    );
    assert!(hits.iter().any(|h| h.matched_in == MatchIn::Column));
    assert!(hits.iter().any(|h| h.matched_in == MatchIn::Definition), "{hits:?}");

    let dbs = conn.list_databases().await.expect("databases");
    assert!(dbs.iter().any(|d| d.eq_ignore_ascii_case(&cat.database)), "{dbs:?}");
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
    // server errors formatted as "Msg <number>, Level …"
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
        conn.execute("WAITFOR DELAY '00:00:30'", &Limits::default()).await
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
    let out = conn.execute("CALL touch_customer(1)", &Limits::default()).await;
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
    let out = conn.execute("SELECT pg_sleep(30)", &Limits::default()).await;
    canceller.await.unwrap();
    assert!(out.failed && first_error(&out).contains("cancel"), "{out:?}");
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
        Names {
            customers: "customers",
            orders: "orders",
            customers_table: "customers",
            orders_table: "orders",
            fk_column: "customer_id",
        },
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
    let options = sqlotus_db::ScriptOptions {
        counted: true,
        ..Default::default()
    };
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
    let plan = conn
        .explain(&format!("SELECT name FROM {customers} WHERE name LIKE 'A%'"))
        .await;
    assert!(plan.errors.is_empty(), "{:?}", plan.errors);
    assert_eq!(plan.plans.len(), 1, "{plan:#?}");
    assert!(plan.plans[0].root.is_some());
    // nothing ran and the session is back to normal
    assert_eq!(
        conn.query_strings("SELECT 1").await.unwrap()[0][0].as_deref(),
        Some("1")
    );

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
        assert_eq!(
            out[0].output.result_sets.last().unwrap().rows[0][0].as_deref(),
            Some("6")
        );
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

#[tokio::test]
async fn postgres_failed_transaction_is_detected() {
    let Some(e) = env("SQLOTUS_TEST_PG") else {
        return;
    };
    let mut conn = connect(&e).await;
    let limits = Limits::default();
    assert!(!conn.execute("BEGIN", &limits).await.failed);
    assert!(conn.execute("SELECT 1/0", &limits).await.failed);
    // a failed transaction still counts as open, and other queries are refused
    assert_eq!(conn.open_transactions().await.unwrap(), 1);
    let probe = conn.execute("SELECT 1", &limits).await;
    assert!(
        probe
            .messages
            .iter()
            .any(|m| m.text.contains("current transaction is aborted")),
        "{probe:?}"
    );
    assert!(!conn.execute("ROLLBACK", &limits).await.failed);
    assert_eq!(conn.open_transactions().await.unwrap(), 0);
}

/// A read-only PostgreSQL / MySQL session refuses writes on the server, even
/// when the statement reaches the driver without the keyword check.
async fn read_only_session_refuses_writes(var: &str, refused: &str) {
    let Some(mut e) = env(var) else {
        return;
    };
    e.config.read_only = true;
    let mut conn = connect(&e).await;
    let limits = Limits::default();
    let read = conn.execute("SELECT COUNT(*) FROM customers", &limits).await;
    assert!(!read.failed, "{read:?}");
    let write = conn
        .execute("UPDATE customers SET name = name WHERE 1 = 0", &limits)
        .await;
    assert!(write.failed, "write was not refused: {write:?}");
    assert!(first_error(&write).to_lowercase().contains(refused), "{write:?}");
}

#[tokio::test]
async fn postgres_read_only_session() {
    read_only_session_refuses_writes("SQLOTUS_TEST_PG", "read-only transaction").await;
}

#[tokio::test]
async fn mysql_read_only_session() {
    read_only_session_refuses_writes("SQLOTUS_TEST_MYSQL", "read only transaction").await;
}

/// A script with `DELIMITER` lines (as from a dump or a tutorial) creates the
/// procedure and trigger, and the statements after `DELIMITER ;` still run.
#[tokio::test]
async fn mysql_delimiter_script() {
    let Some(e) = env("SQLOTUS_TEST_MYSQL") else {
        return;
    };
    let mut conn = connect(&e).await;
    let limits = Limits::default();
    let script = "DROP PROCEDURE IF EXISTS sqlotus_delim_p;\n\
                  DROP TABLE IF EXISTS sqlotus_delim_t;\n\
                  CREATE TABLE sqlotus_delim_t (a int);\n\
                  DELIMITER //\n\
                  CREATE PROCEDURE sqlotus_delim_p(IN n int)\nBEGIN\n  INSERT INTO sqlotus_delim_t VALUES (n);\n  SELECT COUNT(*) AS c FROM sqlotus_delim_t;\nEND //\n\
                  CREATE TRIGGER sqlotus_delim_tr BEFORE INSERT ON sqlotus_delim_t FOR EACH ROW BEGIN SET NEW.a = NEW.a * 10; END//\n\
                  -- a comment after the last statement is not sent on its own (an empty query)\n\
                  DELIMITER ;\n\
                  CALL sqlotus_delim_p(4);\n\
                  SELECT a FROM sqlotus_delim_t;";
    let out = execute_script(&mut conn, script, &limits).await;
    assert!(errors(&out).is_empty(), "{:?}", errors(&out));
    let last = out.last().unwrap();
    let rows = &last.output.result_sets.last().expect("result set").rows;
    assert_eq!(rows.len(), 1);
    assert_eq!(rows[0][0].as_deref(), Some("40"), "the trigger ran");
    execute_script(
        &mut conn,
        "DROP PROCEDURE sqlotus_delim_p; DROP TABLE sqlotus_delim_t;",
        &limits,
    )
    .await;
}

/// String literals built by Sqlotus come back from each server character for
/// character: quotes, backslashes, Vietnamese and an emoji.
async fn literals_round_trip(var: &str, dialect: sqlotus_core::Dialect) {
    let Some(e) = env(var) else {
        return;
    };
    let mut conn = connect(&e).await;
    let value = "C:\temp\new 'quoted' \"double\" Nguyễn 😀 %_ \\\\";
    let mut lit = dialect.string_literal(value);
    if dialect == sqlotus_core::Dialect::MsSql {
        lit = format!("N{lit}");
    }
    let out = conn.execute(&format!("SELECT {lit} AS v"), &Limits::default()).await;
    assert!(!out.failed, "{out:?}");
    assert_eq!(out.result_sets[0].rows[0][0].as_deref(), Some(value));
}

#[tokio::test]
async fn mssql_literals_round_trip() {
    literals_round_trip("SQLOTUS_TEST_MSSQL", sqlotus_core::Dialect::MsSql).await;
}

#[tokio::test]
async fn postgres_literals_round_trip() {
    literals_round_trip("SQLOTUS_TEST_PG", sqlotus_core::Dialect::Postgres).await;
}

#[tokio::test]
async fn mysql_literals_round_trip() {
    literals_round_trip("SQLOTUS_TEST_MYSQL", sqlotus_core::Dialect::MySql).await;
}

/// A value longer than the display limit is cut and counted, so the grid can
/// warn before it is edited or exported.
async fn long_values_are_counted(var: &str, sql: &str) {
    let Some(e) = env(var) else {
        return;
    };
    let mut conn = connect(&e).await;
    let out = conn.execute(sql, &Limits::default()).await;
    assert!(!out.failed, "{out:?}");
    let rs = &out.result_sets[0];
    assert_eq!(rs.clipped, 1);
    let v = rs.rows[0][0].as_deref().unwrap();
    assert_eq!(v.chars().count(), 65_536, "65,535 characters and the ellipsis");
    assert!(v.ends_with('…'));
    assert_eq!(rs.rows[0][1].as_deref(), Some("short"), "short values are not cut");
}

#[tokio::test]
async fn mssql_long_values_are_counted() {
    long_values_are_counted(
        "SQLOTUS_TEST_MSSQL",
        "SELECT REPLICATE(CAST('a' AS nvarchar(max)), 70000) AS v, 'short' AS s",
    )
    .await;
}

#[tokio::test]
async fn postgres_long_values_are_counted() {
    long_values_are_counted("SQLOTUS_TEST_PG", "SELECT repeat('a', 70000) AS v, 'short' AS s").await;
}

#[tokio::test]
async fn mysql_long_values_are_counted() {
    long_values_are_counted("SQLOTUS_TEST_MYSQL", "SELECT REPEAT('a', 70000) AS v, 'short' AS s").await;
}

/// Saving edited rows and the large-change check count changed rows; NOCOUNT,
/// set by the session or by the script, must not hide them (0 rows would
/// read as "changed by someone else", or skip the confirmation).
#[tokio::test]
async fn mssql_changes_are_counted_despite_nocount() {
    let Some(e) = env("SQLOTUS_TEST_MSSQL") else {
        return;
    };
    let mut c = connect(&e).await;
    let l = Limits::default();
    c.execute("CREATE TABLE #t (id int); INSERT #t VALUES (1), (2), (3);", &l)
        .await;
    let a = c.execute_counted("UPDATE #t SET id = id WHERE id <= 2", &l).await;
    assert_eq!(a.rows_affected, 2);
    let b = c
        .execute_counted(
            "SET NOCOUNT ON; UPDATE #t SET id = id WHERE id <= 2; set   nocount  on\nDELETE #t WHERE id = 3",
            &l,
        )
        .await;
    assert_eq!(b.rows_affected, 3, "{b:?}");
    c.execute("SET NOCOUNT ON", &l).await;
    let d = c.execute_counted("UPDATE #t SET id = id WHERE id <= 2", &l).await;
    assert_eq!(d.rows_affected, 2);
    // the session keeps its own setting
    let rows = c.query_strings("SELECT CAST(@@OPTIONS & 512 AS int)").await.unwrap();
    assert_eq!(rows[0][0].as_deref(), Some("512"), "NOCOUNT is still on in the session");
}

/// A procedure named first in a batch runs without EXEC. What Sqlotus puts
/// in front of the batch, values for its variables or the row count, must
/// keep it a call (it read as a syntax error).
#[tokio::test]
async fn mssql_procedure_named_first_in_a_batch() {
    let Some(e) = env("SQLOTUS_TEST_MSSQL") else {
        return;
    };
    let mut c = connect(&e).await;
    let l = Limits::default();
    let made = c
        .execute("CREATE PROCEDURE #plus_one @a int AS SELECT @a + 1 AS v", &l)
        .await;
    assert!(!made.failed, "{made:?}");
    let value = |out: &BatchOutput| {
        out.result_sets
            .last()
            .and_then(|rs| rs.rows.first())
            .and_then(|r| r[0].clone())
    };
    // after another statement, the server reads the name as no call
    assert!(c.execute("SET NOCOUNT OFF; #plus_one 1", &l).await.failed);
    // @a is the procedure's parameter: only @x needs a value
    let script = "#plus_one @a = @x";
    assert_eq!(
        sqlotus_core::variables::undeclared_variables(script, sqlotus_core::Dialect::MsSql),
        vec!["@x"]
    );
    for counted in [false, true] {
        let options = sqlotus_db::ScriptOptions {
            variables: vec![sqlotus_core::variables::VariableValue {
                name: "@x".into(),
                data_type: "int".into(),
                value: Some("41".into()),
            }],
            counted,
        };
        let out = sqlotus_db::execute_script_with(&mut c, script, &l, &options).await;
        assert!(errors(&out).is_empty(), "counted {counted}: {:?}", errors(&out));
        assert_eq!(value(&out[0].output).as_deref(), Some("42"));
    }
    let counted = c.execute_counted("-- the first line\n#plus_one 1", &l).await;
    assert_eq!(first_error(&counted), "");
    assert_eq!(value(&counted).as_deref(), Some("2"));
}

/// A port typed into the host field (`host:port`, `[::1]:port`) connects
/// like the Port field; it used to be looked up as a server name.
async fn port_in_host_field(var: &str) {
    let Some(e) = env(var) else {
        return;
    };
    let port = e.config.port.unwrap_or(e.config.default_port());
    let host = e.config.endpoint().host;
    let host = if host == "localhost" {
        "127.0.0.1".to_string()
    } else {
        host
    };
    for typed in [format!("{host}:{port}"), format!("[::1]:{port}")] {
        if typed.starts_with('[') && host != "127.0.0.1" {
            continue;
        }
        let mut cfg = e.config.clone();
        cfg.host = typed.clone();
        cfg.port = None;
        let mut c = Connection::open(&cfg, e.password.as_deref())
            .await
            .unwrap_or_else(|err| panic!("{typed}: {err}"));
        let rows = c.query_strings("SELECT 1").await.unwrap();
        assert_eq!(rows[0][0].as_deref(), Some("1"), "{typed}");
        c.close().await;
    }
}

#[tokio::test]
async fn postgres_port_in_host_field() {
    port_in_host_field("SQLOTUS_TEST_PG").await;
}

#[tokio::test]
async fn mysql_port_in_host_field() {
    port_in_host_field("SQLOTUS_TEST_MYSQL").await;
}

/// Execute current statement (Shift+F5) inside a MySQL procedure written
/// without DELIMITER: the whole definition is sent, so the procedure is
/// created and no statement of its body runs on its own.
#[tokio::test]
async fn mysql_current_statement_inside_a_procedure() {
    let Some(e) = env("SQLOTUS_TEST_MYSQL") else {
        return;
    };
    let mut c = connect(&e).await;
    let l = Limits::default();
    let setup = "DROP PROCEDURE IF EXISTS sqlotus_clear_staging; DROP TABLE IF EXISTS sqlotus_staging; \
                 CREATE TABLE sqlotus_staging (id int); INSERT INTO sqlotus_staging VALUES (1), (2);";
    assert!(!c.execute(setup, &l).await.failed);
    // the DELETE is the body's second statement: on its own it would empty the table
    let script = "SELECT 1;\nCREATE PROCEDURE sqlotus_clear_staging()\nBEGIN\n  SELECT 'clearing';\n  DELETE FROM sqlotus_staging;\nEND;\nSELECT 2;";
    let at = script.find("DELETE").unwrap() + 2;
    let span = sqlotus_core::statements::executable_statement_at(script, sqlotus_core::Dialect::MySql, at).unwrap();
    let out = c.execute(span.text(script), &l).await;
    assert!(!out.failed, "{:?}", out.messages);
    let rows = c.query_strings("SELECT COUNT(*) FROM sqlotus_staging").await.unwrap();
    assert_eq!(rows[0][0].as_deref(), Some("2"), "the body did not run");
    let exists = c
        .query_strings("SELECT COUNT(*) FROM information_schema.ROUTINES WHERE ROUTINE_NAME = 'sqlotus_clear_staging'")
        .await
        .unwrap();
    assert_eq!(exists[0][0].as_deref(), Some("1"));
    c.execute("DROP PROCEDURE sqlotus_clear_staging; DROP TABLE sqlotus_staging;", &l)
        .await;
}
