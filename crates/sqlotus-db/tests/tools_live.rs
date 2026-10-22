//! The scripts Sqlotus writes from a database, run against real servers
//! (skipped unless the SQLOTUS_TEST_* variables are set, see `live.rs`):
//! a table's data as INSERT statements brings every row back exactly,
//! identity values included; the value search finds what was planted and
//! nothing that only looks like it; SQLCMD variables reach the server; a
//! transaction tells whether it wrote anything; and the backup dialog's
//! scripts back up a SQL Server database and restore it as a copy.

use serde::Deserialize;
use sqlotus_core::Dialect;
use sqlotus_core::backup::{
    BackupFile, BackupKind, BackupOptions, RestoreOptions, backup_script, default_moves, restore_script,
};
use sqlotus_core::catalog::DbObject;
use sqlotus_core::datascript::{DataScriptOptions, data_query, data_script};
use sqlotus_core::findvalue::{FindValueOptions, MatchMode, find_value_script};
use sqlotus_db::{BatchOutput, Connection, ConnectionConfig, Limits, execute_script, load_catalog};

#[derive(Deserialize)]
struct Env {
    #[serde(flatten)]
    config: ConnectionConfig,
    password: Option<String>,
}

fn env(var: &str) -> Option<Env> {
    serde_json::from_str(&std::env::var(var).ok()?).ok()
}

async fn connect(e: &Env, database: Option<&str>) -> Connection {
    let mut cfg = e.config.clone();
    if let Some(db) = database {
        cfg.database = Some(db.to_string());
    }
    Connection::open(&cfg, e.password.as_deref()).await.expect("connect")
}

fn ok(out: &BatchOutput, what: &str) {
    assert!(!out.failed, "{what}: {}", out.errors());
}

/// A new, empty test database, and a connection to it.
async fn fresh(e: &Env, db: &str) -> Connection {
    let l = Limits::default();
    let mut admin = connect(e, None).await;
    let statements = match e.config.dialect {
        Dialect::MsSql => vec![format!(
            "IF DB_ID('{db}') IS NOT NULL BEGIN ALTER DATABASE [{db}] SET SINGLE_USER WITH ROLLBACK IMMEDIATE; DROP DATABASE [{db}]; END; CREATE DATABASE [{db}];"
        )],
        Dialect::Postgres => vec![
            format!("DROP DATABASE IF EXISTS {db} WITH (FORCE)"),
            format!("CREATE DATABASE {db}"),
        ],
        Dialect::MySql => vec![format!("DROP DATABASE IF EXISTS {db}; CREATE DATABASE {db}")],
    };
    for s in statements {
        ok(&admin.execute(&s, &l).await, &s);
    }
    admin.close().await;
    connect(e, Some(db)).await
}

/// A table of every common type with an identity key, a computed column and
/// rows with a gap in the keys, as `(statements, columns compared)`.
fn typed_table(d: Dialect) -> (Vec<&'static str>, &'static str) {
    match d {
        Dialect::MsSql => (
            vec![
                "CREATE TABLE dbo.Typed (id int IDENTITY(1,1) NOT NULL PRIMARY KEY, i int, bi bigint, b bit, d decimal(18,4), \
                 m money, f float, r real, dt date, dtm datetime, dt2 datetime2(7), sdt smalldatetime, dto datetimeoffset(7), \
                 t time(7), c char(5), vc varchar(50), nvc nvarchar(50), nvmax nvarchar(max), tx ntext, u uniqueidentifier, \
                 vb varbinary(16), bn binary(4), x xml, twice AS (i * 2), rv rowversion)",
                "INSERT INTO dbo.Typed (i, bi, b, d, m, f, r, dt, dtm, dt2, sdt, dto, t, c, vc, nvc, nvmax, tx, u, vb, bn, x) VALUES \
                 (-5, 9223372036854775807, 1, 12345678901234.5678, 1234567.8912, 0.1, 3.14, '2024-01-12', '2024-01-12 10:20:30.123', \
                 '9999-12-31 23:59:59.9999999', '2024-01-12 10:20:00', '2024-01-31 10:20:30.1234567 +07:00', '23:59:59.9999999', \
                 'ab', 'C:\\temp ''q''', N'Nguyễn 😀', N'line1' + NCHAR(13) + NCHAR(10) + N'line2', N'ntext Ẩn', \
                 '6F9619FF-8B86-D011-B42D-00C04FC964FF', 0x41, 0x01020304, N'<a b=\"1\">Nguyễn</a>'), \
                 (1, NULL, 0, NULL, NULL, NULL, NULL, NULL, NULL, NULL, NULL, NULL, NULL, NULL, NULL, NULL, NULL, NULL, NULL, NULL, NULL, NULL), \
                 (2, 0, NULL, 0, 0, -0.5, NULL, '0001-01-01', '1753-01-01', NULL, NULL, NULL, '00:00:00', '', '', N'', N'', N'', NULL, 0x, NULL, NULL)",
                "DELETE FROM dbo.Typed WHERE id = 2",
            ],
            "id, i, bi, b, d, m, f, r, dt, dtm, dt2, sdt, dto, t, c, vc, nvc, nvmax, CAST(tx AS nvarchar(max)), u, vb, bn, CAST(x AS nvarchar(max)), twice",
        ),
        Dialect::Postgres => (
            vec![
                "CREATE TABLE typed (id integer GENERATED ALWAYS AS IDENTITY PRIMARY KEY, i integer, bi bigint, b boolean, \
                 d numeric(18,4), r real, dp double precision, m money, dt date, ts timestamp(6), tz timestamptz, t time, \
                 iv interval, c char(5), vc varchar(50), tx text, u uuid, by bytea, j json, jb jsonb, ip inet, arr integer[], \
                 tarr text[], x xml, bt bit(1), b8 bit(8), twice integer GENERATED ALWAYS AS (i * 2) STORED, n serial)",
                "INSERT INTO typed (i, bi, b, d, r, dp, m, dt, ts, tz, t, iv, c, vc, tx, u, by, j, jb, ip, arr, tarr, x, bt, b8) VALUES \
                 (-5, 9223372036854775807, true, 12345678901234.5678, 3.14, 0.1, 1234.56, '2024-01-12', '2024-01-31 10:20:30.123456', \
                 '2024-01-31 10:20:30.123456+07', '23:59:59.999999', '1 day 02:03:04', 'ab', 'C:\\temp ''q''', E'Nguyễn 😀\\nline2', \
                 '6f9619ff-8b86-d011-b42d-00c04fc964ff', '\\x00ff10', '{\"a\": [1, 2]}', '{\"b\": \"Việt\"}', '192.168.0.1', '{1,2,3}', \
                 '{\"a b\",\"c,d\"}', '<a>Nguyễn</a>', B'1', B'10101010'), \
                 (1, NULL, false, NULL, NULL, NULL, NULL, NULL, NULL, NULL, NULL, NULL, NULL, NULL, NULL, NULL, NULL, NULL, NULL, NULL, NULL, NULL, NULL, NULL, NULL), \
                 (2, 0, NULL, 0, -0.5, 1e308, -1, '0001-01-01', NULL, NULL, '00:00', '0', '', '', '', NULL, '\\x', '[]', '{}', NULL, '{}', '{}', NULL, B'0', B'00000000')",
                "DELETE FROM typed WHERE id = 2",
            ],
            "id, i, bi, b, d, r, dp, m, dt, ts, tz, t, iv, c, vc, tx, u, by, j::text, jb::text, ip, arr, tarr, x::text, bt, b8, twice, n",
        ),
        Dialect::MySql => (
            vec![
                "CREATE TABLE typed (id int AUTO_INCREMENT PRIMARY KEY, i int, bi bigint, bl tinyint(1), bt bit(1), b8 bit(8), \
                 ub bigint unsigned, d decimal(18,4), f float, dbl double, dt date, dtm datetime(6), t time(6), y year, c char(5), \
                 vc varchar(50), tx text, bn binary(4), vb varbinary(16), bb blob, j json, e enum('a','b'), st set('x','y'), \
                 twice int GENERATED ALWAYS AS (i * 2) VIRTUAL, created timestamp NULL DEFAULT CURRENT_TIMESTAMP)",
                "INSERT INTO typed (i, bi, bl, bt, b8, ub, d, f, dbl, dt, dtm, t, y, c, vc, tx, bn, vb, bb, j, e, st, created) VALUES \
                 (-5, 9223372036854775807, 1, b'1', b'10101010', 18446744073709551615, 12345678901234.5678, 3.14, 0.1, '2024-01-12', \
                 '2024-01-31 10:20:30.123456', '838:59:59', 2024, 'ab', 'C:\\\\temp ''q''', 'Nguyễn 😀', 0x00FF1000, 0x41, 0x0102, \
                 '{\"a\": [1, 2]}', 'b', 'x,y', '2001-02-03 04:05:06'), \
                 (1, NULL, NULL, NULL, NULL, NULL, NULL, NULL, NULL, NULL, NULL, NULL, NULL, NULL, NULL, NULL, NULL, NULL, NULL, NULL, NULL, NULL, NULL), \
                 (2, 0, 0, b'0', b'0', 0, -0.5, -1.5, 1e300, '1000-01-01', '1000-01-01 00:00:00', '-838:59:59', 1901, '', '', '', 0x00000000, X'', '', '[]', 'a', '', '2001-02-03 04:05:06')",
                "DELETE FROM typed WHERE id = 2",
            ],
            "id, i, bi, bl, bt, b8, ub, d, f, dbl, dt, dtm, t, y, c, vc, tx, bn, vb, bb, j, e, st, twice, created",
        ),
    }
}

fn find<'a>(objects: &'a [DbObject], name: &str) -> &'a DbObject {
    objects
        .iter()
        .find(|o| o.name.eq_ignore_ascii_case(name))
        .unwrap_or_else(|| panic!("{name} not loaded"))
}

async fn data_round_trip(var: &str) {
    let Some(e) = env(var) else { return };
    let d = e.config.dialect;
    let l = Limits::default();
    let mut conn = fresh(&e, "sqlotus_tools").await;
    let (statements, columns) = typed_table(d);
    for s in &statements {
        ok(&conn.execute(s, &l).await, s);
    }
    let table = if d == Dialect::MsSql { "dbo.Typed" } else { "typed" };
    let read = format!("SELECT {columns} FROM {table} ORDER BY id");
    let before = conn.query_strings(&read).await.unwrap();
    assert_eq!(before.len(), 2);

    let catalog = load_catalog(&mut conn).await.unwrap().catalog;
    let t = find(&catalog.objects, "typed").clone();
    let opts = DataScriptOptions {
        max_rows: 100,
        filter: String::new(),
    };
    let rows = conn.query_strings(&data_query(&t, d, &opts).unwrap()).await.unwrap();
    let script = data_script(&t, d, &rows, opts.max_rows);
    assert!(
        script.problems.is_empty(),
        "{d:?}: {:?}\n{}",
        script.problems,
        script.script
    );
    assert_eq!(script.rows, 2);

    // the rows go, and come back from the script (SQL Server read dates as dmy)
    ok(&conn.execute(&format!("DELETE FROM {table}"), &l).await, "delete");
    if d == Dialect::MsSql {
        ok(&conn.execute("SET DATEFORMAT dmy", &l).await, "dateformat");
    }
    for b in execute_script(&mut conn, &script.script, &l).await {
        assert!(!b.output.failed, "{d:?}: {}\n{}", b.output.errors(), script.script);
    }
    let after = conn.query_strings(&read).await.unwrap();
    for (row_before, row_after) in before.iter().zip(&after) {
        for (k, (x, y)) in row_before.iter().zip(row_after).enumerate() {
            assert_eq!(
                x,
                y,
                "{d:?}: column {} of row {:?}",
                columns.split(", ").nth(k).unwrap(),
                row_before[0]
            );
        }
    }
    assert_eq!(before.len(), after.len());

    // new rows get keys after the scripted ones
    let insert = match d {
        Dialect::MsSql => "INSERT INTO dbo.Typed (i) VALUES (7)",
        _ => "INSERT INTO typed (i) VALUES (7)",
    };
    ok(&conn.execute(insert, &l).await, "insert after");
    let next = conn
        .query_strings(&format!("SELECT MAX(id) FROM {table}"))
        .await
        .unwrap();
    let next: i64 = next[0][0].as_deref().unwrap().parse().unwrap();
    assert!(next > 3, "{d:?}: the next key is {next}");

    // a condition picks the rows; one that changes something is refused
    let only = DataScriptOptions {
        max_rows: 1,
        filter: "i = -5".into(),
    };
    let rows = conn.query_strings(&data_query(&t, d, &only).unwrap()).await.unwrap();
    let one = data_script(&t, d, &rows, 1);
    assert_eq!((one.rows, one.truncated), (1, false));
    let all = conn
        .query_strings(
            &data_query(
                &t,
                d,
                &DataScriptOptions {
                    max_rows: 1,
                    filter: String::new(),
                },
            )
            .unwrap(),
        )
        .await
        .unwrap();
    assert!(data_script(&t, d, &all, 1).truncated);
    conn.close().await;
}

#[tokio::test]
async fn mssql_table_data_as_insert() {
    data_round_trip("SQLOTUS_TEST_MSSQL").await;
}

#[tokio::test]
async fn postgres_table_data_as_insert() {
    data_round_trip("SQLOTUS_TEST_PG").await;
}

#[tokio::test]
async fn mysql_table_data_as_insert() {
    data_round_trip("SQLOTUS_TEST_MYSQL").await;
}

/// `(table, column, matches)` the search script finds.
async fn search(conn: &mut Connection, d: Dialect, opts: &FindValueOptions) -> Vec<(String, String, String)> {
    let catalog = load_catalog(conn).await.unwrap().catalog;
    let script = find_value_script(&catalog, d, opts).unwrap();
    let mut out = Vec::new();
    for b in execute_script(conn, &script, &Limits::default()).await {
        assert!(!b.output.failed, "{d:?}: {}\n{script}", b.output.errors());
        for rs in b.output.result_sets {
            for r in rs.rows {
                let cell = |i: usize| r[i].clone().unwrap_or_default();
                out.push((cell(0), cell(1), cell(2)));
            }
        }
    }
    out.sort();
    out
}

async fn value_search(var: &str) {
    let Some(e) = env(var) else { return };
    let d = e.config.dialect;
    let l = Limits::default();
    let mut conn = fresh(&e, "sqlotus_find").await;
    let (schema, setup) = match d {
        Dialect::MsSql => (
            "dbo",
            "CREATE TABLE dbo.Customers (Id int PRIMARY KEY, Name nvarchar(100), Note ntext);
             CREATE TABLE dbo.Orders (Id int PRIMARY KEY, Code varchar(20), Total int);
             INSERT INTO dbo.Customers VALUES (1, N'a 50% needle_x', N'nothing'), (2, N'500 needleZ', N'Ẩn needle 50% needle_');
             INSERT INTO dbo.Orders VALUES (1, 'NEEDLE', 5), (2, 'hay', 6);",
        ),
        Dialect::Postgres => (
            "public",
            "CREATE TABLE customers (id int PRIMARY KEY, name varchar(100), note text);
             CREATE TABLE orders (id int PRIMARY KEY, code char(20), total int);
             INSERT INTO customers VALUES (1, 'a 50% needle_x', 'nothing'), (2, '500 needleZ', 'Ẩn needle 50% needle_');
             INSERT INTO orders VALUES (1, 'NEEDLE', 5), (2, 'hay', 6);",
        ),
        Dialect::MySql => (
            "",
            "CREATE TABLE customers (id int PRIMARY KEY, name varchar(100), note text);
             CREATE TABLE orders (id int PRIMARY KEY, code varchar(20), total int);
             INSERT INTO customers VALUES (1, 'a 50% needle_x', 'nothing'), (2, '500 needleZ', 'Ẩn needle 50% needle_');
             INSERT INTO orders VALUES (1, 'NEEDLE', 5), (2, 'hay', 6);",
        ),
    };
    for b in execute_script(&mut conn, setup, &l).await {
        ok(&b.output, "setup");
    }
    let table = |t: &str| {
        let t = if d == Dialect::MsSql {
            t.to_string()
        } else {
            t.to_lowercase()
        };
        match d {
            Dialect::MySql => format!("sqlotus_find.{t}"),
            _ => format!("{schema}.{t}"),
        }
    };
    let col = |c: &str| {
        if d == Dialect::MsSql {
            c.to_string()
        } else {
            c.to_lowercase()
        }
    };
    let hit = |t: &str, c: &str, n: &str| (table(t), col(c), n.to_string());
    let opts = |value: &str, mode: MatchMode, ignore_case: bool| FindValueOptions {
        value: value.into(),
        mode,
        schema: String::new(),
        ignore_case,
    };
    // wildcards in the value are plain characters: "500 needleZ" does not match
    assert_eq!(
        search(&mut conn, d, &opts("50% needle_", MatchMode::Contains, true)).await,
        vec![hit("Customers", "Name", "1"), hit("Customers", "Note", "1")]
    );
    let needle = search(&mut conn, d, &opts("needle", MatchMode::Contains, true)).await;
    assert_eq!(
        needle,
        vec![
            hit("Customers", "Name", "2"),
            hit("Customers", "Note", "1"),
            hit("Orders", "Code", "1")
        ]
    );
    if d == Dialect::Postgres {
        // case matters in PostgreSQL unless asked
        assert_eq!(
            search(&mut conn, d, &opts("needle", MatchMode::Contains, false)).await,
            vec![hit("Customers", "Name", "2"), hit("Customers", "Note", "1")]
        );
    }
    assert_eq!(
        search(&mut conn, d, &opts("500 needleZ", MatchMode::Exact, true)).await,
        vec![hit("Customers", "Name", "1")]
    );
    assert_eq!(
        search(&mut conn, d, &opts("Ẩn", MatchMode::StartsWith, true)).await,
        vec![hit("Customers", "Note", "1")]
    );
    assert!(
        search(&mut conn, d, &opts("not there", MatchMode::Contains, true))
            .await
            .is_empty()
    );
    conn.close().await;
}

#[tokio::test]
async fn mssql_find_a_value() {
    value_search("SQLOTUS_TEST_MSSQL").await;
}

#[tokio::test]
async fn postgres_find_a_value() {
    value_search("SQLOTUS_TEST_PG").await;
}

#[tokio::test]
async fn mysql_find_a_value() {
    value_search("SQLOTUS_TEST_MYSQL").await;
}

async fn wrote(var: &str) {
    let Some(e) = env(var) else { return };
    let d = e.config.dialect;
    let l = Limits::default();
    let mut conn = fresh(&e, "sqlotus_wrote").await;
    ok(&conn.execute("CREATE TABLE w (a int)", &l).await, "create");
    // SQL Server logs something in the first transactions of a database
    // created a moment ago: a false "wrote" (one question too many) until
    // it settles
    let mut settled = false;
    for _ in 0..20 {
        ok(&conn.execute(conn.begin_statement(), &l).await, "begin");
        ok(&conn.execute("SELECT COUNT(*) FROM w", &l).await, "select");
        if conn.transaction_wrote().await == Some(false) {
            settled = true;
            break;
        }
        ok(&conn.execute("ROLLBACK", &l).await, "rollback");
        tokio::time::sleep(std::time::Duration::from_millis(200)).await;
    }
    assert!(settled, "{d:?}: a transaction that only read counted as writing");
    ok(&conn.execute("INSERT INTO w VALUES (1)", &l).await, "insert");
    assert_eq!(conn.transaction_wrote().await, Some(true), "{d:?}: wrote a row");
    ok(&conn.execute("ROLLBACK", &l).await, "rollback");
    conn.close().await;
}

#[tokio::test]
async fn mssql_transaction_wrote() {
    wrote("SQLOTUS_TEST_MSSQL").await;
}

#[tokio::test]
async fn postgres_transaction_wrote() {
    wrote("SQLOTUS_TEST_PG").await;
}

#[tokio::test]
async fn mysql_transaction_wrote() {
    wrote("SQLOTUS_TEST_MYSQL").await;
}

#[tokio::test]
async fn mssql_sqlcmd_variables() {
    let Some(e) = env("SQLOTUS_TEST_MSSQL") else { return };
    let l = Limits::default();
    let mut conn = fresh(&e, "sqlotus_sqlcmd").await;
    ok(
        &conn
            .execute(
                "CREATE TABLE dbo.Orders_2024 (Id int); INSERT INTO dbo.Orders_2024 VALUES (1), (2)",
                &l,
            )
            .await,
        "setup",
    );
    let script = ":setvar Year 2024\n:setvar Who \"Phạm Tuấn\"\n:on error exit\nSELECT N'$(Who)' AS who, COUNT(*) AS n FROM dbo.Orders_$(Year);\nGO\nSELECT 1 / 0;\nGO\nSELECT 'not reached';";
    let out = execute_script(&mut conn, script, &l).await;
    assert_eq!(out.len(), 2, "stops at the failed batch");
    let rows = &out[0].output.result_sets[0].rows;
    assert_eq!(rows[0], vec![Some("Phạm Tuấn".to_string()), Some("2".to_string())]);
    assert!(out[1].output.failed);
    // without :on error exit, later batches still run
    let out = execute_script(&mut conn, ":setvar x 1\nSELECT $(x) / 0\nGO\nSELECT $(x) AS one", &l).await;
    assert_eq!(out.len(), 2);
    assert_eq!(out[1].output.result_sets[0].rows[0][0].as_deref(), Some("1"));
    // what Sqlotus cannot do is not sent at all
    let out = execute_script(&mut conn, ":setvar x 1\nSELECT 1\nGO\n:r other.sql", &l).await;
    assert_eq!(out.len(), 1);
    assert!(out[0].output.failed && out[0].output.errors().contains(":r is not supported"));
    let out = execute_script(&mut conn, ":setvar x 1\nSELECT $(y)", &l).await;
    assert!(out[0].output.errors().contains("$(y) has no value"));
    // the estimated plan reads the variables too
    let plan = conn.explain(":setvar T Orders_2024\nSELECT * FROM dbo.$(T)").await;
    assert!(plan.errors.is_empty() && plan.plans.len() == 1, "{:?}", plan.errors);
    conn.close().await;
}

#[tokio::test]
async fn mssql_backup_and_restore_as_a_copy() {
    let Some(e) = env("SQLOTUS_TEST_MSSQL") else { return };
    let l = Limits::default();
    let mut conn = fresh(&e, "sqlotus_backup").await;
    ok(
        &conn
            .execute("CREATE TABLE dbo.Kept (Id int PRIMARY KEY, Name nvarchar(20)); INSERT INTO dbo.Kept VALUES (1, N'Một'), (2, N'Hai')", &l)
            .await,
        "setup",
    );
    let dirs = conn
        .query_strings(
            "SELECT CAST(SERVERPROPERTY('InstanceDefaultBackupPath') AS nvarchar(4000)), \
             CAST(SERVERPROPERTY('InstanceDefaultDataPath') AS nvarchar(4000)), CAST(SERVERPROPERTY('InstanceDefaultLogPath') AS nvarchar(4000))",
        )
        .await
        .unwrap();
    let dir = |i: usize| dirs[0][i].clone().unwrap_or_default();
    let folder = if dir(0).is_empty() { dir(1) } else { dir(0) };
    let sep = if folder.contains('\\') { "\\" } else { "/" };
    let path = format!("{}{}sqlotus_backup_test.bak", folder.trim_end_matches(['/', '\\']), sep);
    let backup = backup_script(&BackupOptions {
        database: "sqlotus_backup".into(),
        kind: BackupKind::Full,
        path: path.clone(),
        copy_only: true,
        compression: false,
        checksum: true,
        overwrite: true,
        verify: true,
    })
    .unwrap();
    for b in execute_script(&mut conn, &backup, &l).await {
        ok(&b.output, &backup);
    }
    let list = conn
        .execute(&format!("RESTORE FILELISTONLY FROM DISK = N'{path}'"), &l)
        .await;
    ok(&list, "file list");
    let rs = &list.result_sets[0];
    let at = |name: &str| rs.columns.iter().position(|c| c.name == name).unwrap();
    let files: Vec<BackupFile> = rs
        .rows
        .iter()
        .map(|r| BackupFile {
            logical_name: r[at("LogicalName")].clone().unwrap(),
            physical_name: r[at("PhysicalName")].clone().unwrap(),
            kind: r[at("Type")].clone().unwrap(),
        })
        .collect();
    let moves = default_moves(&files, "sqlotus_backup_copy", &dir(1), &dir(2));
    assert_eq!(moves.len(), 2);
    let restore = restore_script(&RestoreOptions {
        path,
        file_number: 1,
        database: "sqlotus_backup_copy".into(),
        moves,
        replace: true,
        recovery: true,
        close_connections: true,
    })
    .unwrap();
    for b in execute_script(&mut conn, &restore, &l).await {
        ok(&b.output, &restore);
    }
    let copied = conn
        .query_strings("SELECT COUNT(*), MAX(Name) FROM sqlotus_backup_copy.dbo.Kept")
        .await
        .unwrap();
    assert_eq!(copied[0], vec![Some("2".to_string()), Some("Một".to_string())]);
    // restoring again over the copy, with others connected to it, works too
    let other = connect(&e, Some("sqlotus_backup_copy")).await;
    for b in execute_script(&mut conn, &restore, &l).await {
        ok(&b.output, "restore over a database in use");
    }
    other.close().await;
    ok(
        &conn
            .execute("ALTER DATABASE sqlotus_backup_copy SET SINGLE_USER WITH ROLLBACK IMMEDIATE; DROP DATABASE sqlotus_backup_copy", &l)
            .await,
        "cleanup",
    );
    conn.close().await;
}
