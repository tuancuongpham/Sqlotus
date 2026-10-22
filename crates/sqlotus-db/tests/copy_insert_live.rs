//! "Copy as INSERT" against real servers (skipped unless the SQLOTUS_TEST_*
//! variables are set, see `live.rs`; needs node): a row of every common type
//! is read as the grid reads it, turned into INSERT text by the app's own
//! src/export.ts (scripts/copy-insert.mjs), run into a copy of the table
//! (SQL Server with DATEFORMAT dmy), and read back: every value must come
//! back the same.

use serde::Deserialize;
use sqlotus_core::Dialect;
use sqlotus_db::{Connection, ConnectionConfig, Limits};

#[derive(Deserialize)]
struct Env {
    #[serde(flatten)]
    config: ConnectionConfig,
    password: Option<String>,
}

async fn connect(e: &Env, database: Option<&str>) -> Connection {
    let mut cfg = e.config.clone();
    if let Some(db) = database {
        cfg.database = Some(db.to_string());
    }
    Connection::open(&cfg, e.password.as_deref()).await.expect("connect")
}

const DB: &str = "sqlotus_copy";

fn setup(d: Dialect) -> (String, String, &'static str) {
    match d {
        Dialect::MsSql => (
            "(id int NOT NULL PRIMARY KEY, i int, bi bigint, b bit, d decimal(18,4), m money, f float, r real, \
             dt date, dtm datetime, dt2 datetime2(7), sdt smalldatetime, dto datetimeoffset(7), t time(7), c char(5), \
             vc varchar(50), nvc nvarchar(50), nvmax nvarchar(max), u uniqueidentifier, vb varbinary(16), bn binary(4), \
             x xml, sn sysname NULL)"
                .into(),
            "(id, i, bi, b, d, m, f, r, dt, dtm, dt2, sdt, dto, t, c, vc, nvc, nvmax, u, vb, bn, x, sn) VALUES (1, -5, \
             9223372036854775807, 1, 12345678901234.5678, 1234567.8912, 0.1, 3.14, '2024-01-12', '2024-01-12 10:20:30.123', \
             '9999-12-31 23:59:59.9999999', '2024-01-12 10:20:00', '2024-01-31 10:20:30.1234567 +07:00', '23:59:59.9999999', \
             'ab', 'C:\\temp ''q''', N'Nguyễn 😀', N'line1' + NCHAR(13) + NCHAR(10) + N'line2', \
             '6F9619FF-8B86-D011-B42D-00C04FC964FF', 0x41, 0x01020304, N'<a b=\"1\">Nguyễn</a>', N'bảng_Ẩn')"
                .into(),
            "#Results",
        ),
        Dialect::Postgres => (
            "(id integer PRIMARY KEY, i integer, bi bigint, b boolean, d numeric(18,4), r real, dp double precision, \
             m money, dt date, ts timestamp(6), tz timestamptz, t time, iv interval, c char(5), vc varchar(50), \
             tx text, z varchar(10), u uuid, by bytea, j json, jb jsonb, ip inet, arr integer[], tarr text[], \
             x xml, bt bit(1), b8 bit(8))"
                .into(),
            "(id, i, bi, b, d, r, dp, m, dt, ts, tz, t, iv, c, vc, tx, z, u, by, j, jb, ip, arr, tarr, x, bt, b8) VALUES \
             (1, -5, 9223372036854775807, true, 12345678901234.5678, 3.14, 0.1, 1234.56, '2024-01-12', \
             '2024-01-31 10:20:30.123456', '2024-01-31 10:20:30.123456+07', '23:59:59.999999', '1 day 02:03:04', 'ab', \
             'C:\\temp ''q''', E'Nguyễn 😀\\nline2', '00123', '6f9619ff-8b86-d011-b42d-00c04fc964ff', '\\x00ff10', \
             '{\"a\": [1, 2]}', '{\"b\": \"Việt\"}', '192.168.0.1', '{1,2,3}', '{\"a b\",\"c,d\"}', '<a>Nguyễn</a>', B'1', \
             B'10101010')"
                .into(),
            "results",
        ),
        Dialect::MySql => (
            "(id int PRIMARY KEY, i int, bi bigint, bl tinyint(1), bt bit(1), b8 bit(8), ub bigint unsigned, \
             d decimal(18,4), f float, dbl double, dt date, dtm datetime(6), t time(6), y year, c char(5), vc varchar(50), \
             tx text, z varchar(10), bn binary(4), vb varbinary(16), bb blob, j json, e enum('a','b'), st set('x','y'))"
                .into(),
            "(id, i, bi, bl, bt, b8, ub, d, f, dbl, dt, dtm, t, y, c, vc, tx, z, bn, vb, bb, j, e, st) VALUES (1, -5, \
             9223372036854775807, 1, b'1', b'10101010', 18446744073709551615, 12345678901234.5678, 3.14, 0.1, \
             '2024-01-12', '2024-01-31 10:20:30.123456', '838:59:59', 2024, 'ab', 'C:\\\\temp ''q''', 'Nguyễn 😀', \
             '00123', 0x00FF1000, 0x41, 0x0102, '{\"a\": [1, 2]}', 'b', 'x,y')"
                .into(),
            "results",
        ),
    }
}

async fn check(var: &str) {
    let Ok(raw) = std::env::var(var) else { return };
    let e: Env = serde_json::from_str(&raw).unwrap();
    let d = e.config.dialect;
    let l = Limits::default();
    let mut admin = connect(&e, None).await;
    let recreate = match d {
        Dialect::MsSql => vec![format!(
            "IF DB_ID('{DB}') IS NOT NULL BEGIN ALTER DATABASE [{DB}] SET SINGLE_USER WITH ROLLBACK IMMEDIATE; DROP DATABASE [{DB}]; END; CREATE DATABASE [{DB}];"
        )],
        Dialect::Postgres => vec![
            format!("DROP DATABASE IF EXISTS {DB} WITH (FORCE)"),
            format!("CREATE DATABASE {DB}"),
        ],
        Dialect::MySql => vec![format!("DROP DATABASE IF EXISTS {DB}; CREATE DATABASE {DB}")],
    };
    for s in recreate {
        assert!(!admin.execute(&s, &l).await.failed, "{s}");
    }
    admin.close().await;
    let mut conn = connect(&e, Some(DB)).await;
    let (ddl, insert, target) = setup(d);
    for s in [
        format!("CREATE TABLE typed {ddl}"),
        format!("CREATE TABLE typed_copy {ddl}"),
        format!("INSERT INTO typed {insert}"),
    ] {
        let out = conn.execute(&s, &l).await;
        assert!(!out.failed, "{s}: {:?}", out.messages);
    }
    let before = conn.execute("SELECT * FROM typed", &l).await;
    let rs = &before.result_sets[0];
    let dialect = match d {
        Dialect::MsSql => "mssql",
        Dialect::Postgres => "postgres",
        Dialect::MySql => "mysql",
    };
    let json = serde_json::json!({
        "dialect": dialect,
        "columns": rs.columns.iter().map(|c| serde_json::json!({"name": c.name, "typeName": c.type_name})).collect::<Vec<_>>(),
        "rows": rs.rows,
    });
    let input = std::env::temp_dir().join(format!("sqlotus-copy-{dialect}-{}.json", std::process::id()));
    std::fs::write(&input, json.to_string()).unwrap();
    let helper = concat!(env!("CARGO_MANIFEST_DIR"), "/../../scripts/copy-insert.mjs");
    let node = std::process::Command::new("node")
        .arg(helper)
        .arg(&input)
        .output()
        .expect("node");
    std::fs::remove_file(&input).ok();
    assert!(node.status.success(), "{}", String::from_utf8_lossy(&node.stderr));
    let sql = String::from_utf8(node.stdout)
        .unwrap()
        .replacen(target, "typed_copy", 1);
    if d == Dialect::MsSql {
        conn.execute("SET DATEFORMAT dmy", &l).await;
    }
    let out = conn.execute(&sql, &l).await;
    let mut problems: Vec<String> = out
        .messages
        .iter()
        .filter(|m| m.kind == sqlotus_db::MessageKind::Error)
        .map(|m| m.text.clone())
        .collect();
    let after = conn.execute("SELECT * FROM typed_copy", &l).await;
    match after.result_sets.first().and_then(|r| r.rows.first()) {
        Some(row) => {
            for (j, c) in rs.columns.iter().enumerate() {
                if rs.rows[0][j] != row[j] {
                    problems.push(format!("{}: {:?} came back as {:?}", c.name, rs.rows[0][j], row[j]));
                }
            }
        }
        None => problems.push("no row copied".into()),
    }
    conn.close().await;
    assert!(
        problems.is_empty(),
        "{dialect}: {} problem(s):\n{}",
        problems.len(),
        problems.join("\n")
    );
}

#[tokio::test]
async fn mssql_copy_as_insert() {
    check("SQLOTUS_TEST_MSSQL").await;
}

#[tokio::test]
async fn postgres_copy_as_insert() {
    check("SQLOTUS_TEST_PG").await;
}

#[tokio::test]
async fn mysql_copy_as_insert() {
    check("SQLOTUS_TEST_MYSQL").await;
}
