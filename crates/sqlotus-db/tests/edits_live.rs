//! Editing result rows against real servers (skipped unless the
//! SQLOTUS_TEST_* variables are set, see `live.rs`). A table with the common
//! types of each server gets a row through plain SQL. Then, as the grid does,
//! the row is read back as text and
//!
//! 1. saved with every value retyped as shown: the UPDATE must find the row
//!    (its optimistic check compares what was shown) and change nothing;
//! 2. copied as a new row with the values shown (INSERT): the copy must read
//!    back the same;
//! 3. deleted by the values shown (DELETE);
//! 4. given awkward new values, which must read back as typed;
//! 5. saved after someone else changed it: the save must find no row.
//!
//! Each table has a trigger that writes elsewhere even when no row matched
//! (SQL Server counted those rows as the edit's own).

use std::collections::HashMap;

use serde::Deserialize;
use sqlotus_core::Dialect;
use sqlotus_core::editing::{EditPlan, EditSource, RowEdit, edit_statements, editable_source};
use sqlotus_db::{Connection, ConnectionConfig, Limits, load_catalog};

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

const DB: &str = "sqlotus_edits";

struct Setup {
    /// Statements that create the table (and its types) and insert row 1.
    script: &'static [&'static str],
    table: &'static str,
    /// Columns written by the server (identity, computed, rowversion).
    generated: &'static [&'static str],
    /// New values for step 4: (column, typed, expected read back).
    awkward: &'static [(&'static str, Option<&'static str>, Option<&'static str>)],
}

const MSSQL: Setup = Setup {
    script: &[
        "CREATE TABLE dbo.typed (id int NOT NULL PRIMARY KEY, i int, bi bigint, si smallint, ti tinyint, b bit, \
         d decimal(18,4), n numeric(10,2), m money, sm smallmoney, f float, r real, dt date, dtm datetime, \
         dt2 datetime2(7), sdt smalldatetime, dto datetimeoffset(7), t time(7), c char(5), vc varchar(50), \
         nvc nvarchar(50), nc nchar(5), vmax varchar(max), nvmax nvarchar(max), u uniqueidentifier, \
         vb varbinary(16), bn binary(4), x xml, sn sysname NULL, ident int IDENTITY(1,1), calc AS (i * 2), rv rowversion)",
        "INSERT INTO dbo.typed (id, i, bi, si, ti, b, d, n, m, sm, f, r, dt, dtm, dt2, sdt, dto, t, c, vc, nvc, nc, vmax, \
         nvmax, u, vb, bn, x, sn) VALUES (1, -1000000000, 9223372036854775807, -32768, 255, 1, 12345678901234.5678, \
         -12345678.90, 1234567.8912, -214748.3648, 0.1, 3.14, '0001-01-01', '1753-01-01 00:00:00.003', \
         '9999-12-31 23:59:59.9999999', '2079-06-06 23:59:00', '2024-01-31 10:20:30.1234567 +07:00', \
         '23:59:59.9999999', 'ab', 'it''s', N'Nguyễn 😀', N'ễ', 'a\\b', N'line1' + NCHAR(13) + NCHAR(10) + N'line2', \
         '6F9619FF-8B86-D011-B42D-00C04FC964FF', 0x00FF10, 0x01020304, N'<a b=\"1\">Nguyễn</a>', N'bảng_Ẩn')",
        "CREATE TABLE dbo.audit (n int)",
        "CREATE TRIGGER dbo.typed_audit ON dbo.typed AFTER INSERT, UPDATE, DELETE AS INSERT dbo.audit VALUES (1)",
    ],
    table: "dbo.typed",
    generated: &["ident", "calc", "rv"],
    awkward: &[
        ("vc", Some("C:\\temp\\new 'q' %_"), Some("C:\\temp\\new 'q' %_")),
        ("nvc", Some("Trần \"Thị\" ẞ 😀"), Some("Trần \"Thị\" ẞ 😀")),
        ("nvmax", Some(""), Some("")),
        (
            "x",
            Some("<r>Tiếng Việt &amp; 😀</r>"),
            Some("<r>Tiếng Việt &amp; 😀</r>"),
        ),
        ("sn", Some("Đơn_hàng"), Some("Đơn_hàng")),
        ("d", Some("-0.0001"), Some("-0.0001")),
        ("i", Some("-5"), Some("-5")),
        ("dt", Some("2000-02-29"), Some("2000-02-29")),
        ("b", Some("0"), Some("0")),
        ("vb", Some("0x41"), Some("0x41")),
        ("vb", Some("0x"), Some("0x")),
        ("dtm", Some("2024-01-31 10:20:30.123"), Some("2024-01-31 10:20:30.123")),
        ("sdt", Some("2024-01-31 10:20:00"), Some("2024-01-31 10:20:00")),
        ("u", None, None),
    ],
};

const POSTGRES: Setup = Setup {
    script: &[
        "CREATE TYPE mood AS ENUM ('sad', 'ok')",
        "CREATE TABLE public.typed (id integer PRIMARY KEY, i integer, bi bigint, si smallint, b boolean, \
         d numeric(18,4), n numeric, r real, dp double precision, m money, dt date, ts timestamp(6), \
         tz timestamptz, t time, ttz timetz, iv interval, c char(5), vc varchar(50), tx text, u uuid, \
         by bytea, j json, jb jsonb, ip inet, ci cidr, mac macaddr, arr integer[], tarr text[], e mood, \
         x xml, pt point, bt bit(1), b8 bit(8), vbt varbit(16), \
         ident integer GENERATED ALWAYS AS IDENTITY, calc integer GENERATED ALWAYS AS (i * 2) STORED)",
        "INSERT INTO public.typed (id, i, bi, si, b, d, n, r, dp, m, dt, ts, tz, t, ttz, iv, c, vc, tx, u, by, j, jb, \
         ip, ci, mac, arr, tarr, e, x, pt, bt, b8, vbt) VALUES (1, -1000000000, 9223372036854775807, -32768, true, \
         12345678901234.5678, 3.14159265358979323846, 3.14, 0.1, 1234.56, '0001-01-01', '2024-01-31 10:20:30.123456', \
         '2024-01-31 10:20:30.123456+07', '23:59:59.999999', '10:00:00+07', '1 day 02:03:04', 'ab', 'it''s', \
         E'Nguyễn 😀\\nline2', '6f9619ff-8b86-d011-b42d-00c04fc964ff', '\\x00ff10', '{\"a\": [1, 2]}', \
         '{\"b\": \"Việt\"}', '192.168.0.1', '10.0.0.0/8', '08:00:2b:01:02:03', '{1,2,3}', '{\"a b\",\"c,d\"}', 'ok', \
         '<a>Nguyễn</a>', '(1.5,2)', B'1', B'10101010', B'101')",
        "CREATE TABLE audit (n int)",
        "CREATE FUNCTION audit_change() RETURNS trigger LANGUAGE plpgsql AS $$ BEGIN INSERT INTO audit VALUES (1); \
         RETURN NULL; END $$",
        "CREATE TRIGGER typed_audit AFTER INSERT OR UPDATE OR DELETE ON public.typed FOR EACH STATEMENT \
         EXECUTE FUNCTION audit_change()",
    ],
    table: "public.typed",
    generated: &["ident", "calc"],
    awkward: &[
        ("vc", Some("C:\\temp\\new 'q' %_"), Some("C:\\temp\\new 'q' %_")),
        ("tx", Some("Trần \"Thị\" ẞ 😀"), Some("Trần \"Thị\" ẞ 😀")),
        ("d", Some("-0.0001"), Some("-0.0001")),
        ("b", Some("false"), Some("f")),
        ("bt", Some("0"), Some("0")),
        ("by", Some("\\x41"), Some("\\x41")),
        ("m", Some("12.5"), Some("$12.50")),
        ("b8", Some("00001111"), Some("00001111")),
        ("arr", Some("{4,5}"), Some("{4,5}")),
        ("jb", Some("{\"k\": \"v\"}"), Some("{\"k\": \"v\"}")),
        ("dt", Some("2000-02-29"), Some("2000-02-29")),
        ("u", None, None),
    ],
};

const MYSQL: Setup = Setup {
    script: &[
        "CREATE TABLE typed (id int PRIMARY KEY, i int, bi bigint, si smallint, ti tinyint, bl tinyint(1), bt bit(1), \
         b8 bit(8), ui int unsigned, ub bigint unsigned, d decimal(18,4), f float, dbl double, dt date, dtm datetime(6), \
         ts timestamp(6) NULL, t time(6), y year, c char(5), vc varchar(50), tx text, bn binary(4), vb varbinary(16), \
         bb blob, j json, e enum('a','b'), st set('x','y'), calc int GENERATED ALWAYS AS (i * 2) STORED)",
        "INSERT INTO typed (id, i, bi, si, ti, bl, bt, b8, ui, ub, d, f, dbl, dt, dtm, ts, t, y, c, vc, tx, bn, vb, bb, j, \
         e, st) VALUES (1, -1000000000, 9223372036854775807, -32768, -128, 1, b'1', b'10101010', 4294967295, \
         18446744073709551615, 12345678901234.5678, 3.14, 0.1, '1000-01-01', '2024-01-31 10:20:30.123456', \
         '2024-01-31 10:20:30.123456', '838:59:59', 2024, 'ab', 'it''s \\\\ back', 'Nguyễn 😀', 0x00FF1000, 0x00FF10, \
         0x0102, '{\"a\": [1, 2]}', 'b', 'x,y')",
        "CREATE TABLE audit (n int)",
        "CREATE TRIGGER typed_audit AFTER UPDATE ON typed FOR EACH ROW INSERT INTO audit VALUES (1)",
    ],
    table: "typed",
    generated: &["calc"],
    awkward: &[
        ("vc", Some("C:\\temp\\new 'q' %_"), Some("C:\\temp\\new 'q' %_")),
        ("tx", Some("Trần \"Thị\" ẞ 😀"), Some("Trần \"Thị\" ẞ 😀")),
        ("d", Some("-0.0001"), Some("-0.0001")),
        ("bl", Some("0"), Some("0")),
        ("b8", Some("15"), Some("15")),
        ("bt", Some("0"), Some("0")),
        ("vb", Some("0x41"), Some("0x41")),
        ("bb", Some("0x"), Some("0x")),
        ("st", Some("y"), Some("y")),
        ("dt", Some("2000-02-29"), Some("2000-02-29")),
        ("e", None, None),
    ],
};

type Row = HashMap<String, Option<String>>;

async fn read_row(conn: &mut Connection, table: &str, id: i32) -> Option<Row> {
    let out = conn
        .execute(&format!("SELECT * FROM {table} WHERE id = {id}"), &Limits::default())
        .await;
    let rs = out.result_sets.first()?;
    let row = rs.rows.first()?;
    Some(
        rs.columns
            .iter()
            .map(|c| c.name.clone())
            .zip(row.iter().cloned())
            .collect(),
    )
}

async fn run_plan(conn: &mut Connection, plan: &EditPlan, what: &str, problems: &mut Vec<String>) {
    if !plan.problems.is_empty() {
        problems.push(format!("{what}: refused: {:?}", plan.problems));
        return;
    }
    for stmt in &plan.statements {
        match conn.rows_changed(stmt, &Limits::default()).await {
            Ok(1) => {}
            other => problems.push(format!("{what}: {other:?}\n  {stmt}")),
        }
    }
}

fn differences(what: &str, before: &Row, after: &Row, skip: &[&str], problems: &mut Vec<String>) {
    let mut names: Vec<&String> = before.keys().collect();
    names.sort();
    for n in names {
        if skip.contains(&n.as_str()) {
            continue;
        }
        if before.get(n) != after.get(n) {
            problems.push(format!(
                "{what}: {n} was {:?}, reads back {:?}",
                before[n],
                after.get(n)
            ));
        }
    }
}

async fn check(var: &str, setup: &Setup) {
    let Some(e) = env(var) else {
        eprintln!("{var} not set, skipping");
        return;
    };
    let d = e.config.dialect;
    let limits = Limits::default();
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
        let out = admin.execute(&s, &limits).await;
        assert!(!out.failed, "{s}: {:?}", out.messages);
    }
    admin.close().await;
    let mut conn = connect(&e, Some(DB)).await;
    for s in setup.script {
        let out = conn.execute(s, &limits).await;
        assert!(!out.failed, "{s}: {:?}", out.messages);
    }
    if d == Dialect::MsSql {
        assert!(!conn.execute("SET DATEFORMAT dmy", &limits).await.failed);
    }
    let catalog = load_catalog(&mut conn).await.expect("catalog").catalog;
    let select = format!("SELECT * FROM {}", setup.table);
    let out = conn.execute(&select, &limits).await;
    let names: Vec<String> = out.result_sets[0].columns.iter().map(|c| c.name.clone()).collect();
    let source: EditSource = editable_source(&select, d, &catalog, &names).expect("editable");
    let shown = read_row(&mut conn, setup.table, 1).await.expect("row 1");
    let writable: Vec<String> = source
        .columns
        .iter()
        .filter(|c| !c.read_only && !c.is_key)
        .filter_map(|c| c.column.clone())
        .collect();
    let mut problems = Vec::new();

    // 1. every value retyped as shown
    let values: Row = writable.iter().map(|n| (n.clone(), shown[n].clone())).collect();
    let plan = edit_statements(
        &source,
        &[RowEdit::Update {
            original: shown.clone(),
            values,
        }],
        d,
    );
    run_plan(&mut conn, &plan, "save unchanged", &mut problems).await;
    let after = read_row(&mut conn, setup.table, 1).await.expect("row 1");
    differences("save unchanged", &shown, &after, &["rv"], &mut problems);

    // 2. a copy of the row with the values shown
    let mut values: Row = writable.iter().map(|n| (n.clone(), shown[n].clone())).collect();
    values.insert("id".into(), Some("2".into()));
    let plan = edit_statements(&source, &[RowEdit::Insert { values }], d);
    run_plan(&mut conn, &plan, "copy as new row", &mut problems).await;
    let mut skip = vec!["id"];
    skip.extend(setup.generated);
    match read_row(&mut conn, setup.table, 2).await {
        Some(copy) => {
            differences("copy as new row", &shown, &copy, &skip, &mut problems);
            // 3. delete it by the values shown
            let plan = edit_statements(&source, &[RowEdit::Delete { original: copy }], d);
            run_plan(&mut conn, &plan, "delete", &mut problems).await;
        }
        None => problems.push("copy as new row: not found".into()),
    }

    // 4. awkward values, each on its own
    for (col, typed, expected) in setup.awkward {
        let now = read_row(&mut conn, setup.table, 1).await.expect("row 1");
        let values: Row = [(col.to_string(), typed.map(str::to_string))].into();
        let plan = edit_statements(&source, &[RowEdit::Update { original: now, values }], d);
        let what = format!("set {col} to {typed:?}");
        run_plan(&mut conn, &plan, &what, &mut problems).await;
        let after = read_row(&mut conn, setup.table, 1).await.expect("row 1");
        if after.get(*col) != Some(&expected.map(str::to_string)) {
            problems.push(format!("{what}: reads back {:?}", after.get(*col)));
        }
    }

    // 5. someone else changed the row since it was shown
    let shown = read_row(&mut conn, setup.table, 1).await.expect("row 1");
    let id = d.quote_ident("id");
    let i = d.quote_ident("i");
    let table = setup.table;
    let other = conn
        .execute(&format!("UPDATE {table} SET {i} = 424242 WHERE {id} = 1"), &limits)
        .await;
    assert!(!other.failed, "{:?}", other.messages);
    let values: Row = [("i".to_string(), Some("7".to_string()))].into();
    let plan = edit_statements(
        &source,
        &[RowEdit::Update {
            original: shown,
            values,
        }],
        d,
    );
    for stmt in &plan.statements {
        match conn.rows_changed(stmt, &limits).await {
            Ok(0) => {}
            other => problems.push(format!("save over someone else's change: {other:?}\n  {stmt}")),
        }
    }

    conn.close().await;
    let mut admin = connect(&e, None).await;
    let drop = match d {
        Dialect::MsSql => {
            format!("ALTER DATABASE [{DB}] SET SINGLE_USER WITH ROLLBACK IMMEDIATE; DROP DATABASE [{DB}];")
        }
        Dialect::Postgres => format!("DROP DATABASE IF EXISTS {DB} WITH (FORCE)"),
        Dialect::MySql => format!("DROP DATABASE IF EXISTS {DB}"),
    };
    admin.execute(&drop, &limits).await;
    admin.close().await;
    assert!(
        problems.is_empty(),
        "{} problem(s):\n{}",
        problems.len(),
        problems.join("\n")
    );
}

#[tokio::test]
async fn mssql_edited_rows_round_trip() {
    check("SQLOTUS_TEST_MSSQL", &MSSQL).await;
}

#[tokio::test]
async fn postgres_edited_rows_round_trip() {
    check("SQLOTUS_TEST_PG", &POSTGRES).await;
}

#[tokio::test]
async fn mysql_edited_rows_round_trip() {
    check("SQLOTUS_TEST_MYSQL", &MYSQL).await;
}
