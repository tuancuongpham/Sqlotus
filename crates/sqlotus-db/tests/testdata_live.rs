//! Generated test data against real servers (skipped unless the
//! SQLOTUS_TEST_* variables are set, see `live.rs`): for a parent and a
//! child table with the common types of each server (identity and computed
//! columns, short text, small decimals, a foreign key), the generated INSERT
//! scripts must run without errors for several seeds and add every row.
//! SQL Server runs them with DATEFORMAT dmy, as for a British login.

use serde::Deserialize;
use sqlotus_core::Dialect;
use sqlotus_core::catalog::ObjectKind;
use sqlotus_core::testdata::{DataContext, Generator, TestDataOptions, foreign_keys_of, insert_script, plan_columns};
use sqlotus_db::{Connection, ConnectionConfig, Limits, MessageKind, load_catalog};

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

const DB: &str = "sqlotus_testdata";

fn tables(d: Dialect) -> [&'static str; 2] {
    match d {
        Dialect::MsSql => [
            "CREATE TABLE dbo.Customers (Id int IDENTITY(1,1) PRIMARY KEY, Code varchar(10) NOT NULL, \
             FullName nvarchar(100) NOT NULL, Email varchar(100) NULL, Phone varchar(20) NULL, BirthDate date NULL, \
             Country char(2) NULL, CreatedAt datetime2 NULL, Score decimal(5,2) NULL, Active bit NOT NULL, \
             Note nvarchar(max) NULL, Avatar varbinary(max) NULL, Guid uniqueidentifier NULL, Small smallint NULL, \
             Tiny tinyint NULL, Rate real NULL, Amount money NULL, Ts rowversion, Calc AS (Score * 2))",
            "CREATE TABLE dbo.Orders (Id int IDENTITY(1,1) PRIMARY KEY, CustomerId int NOT NULL REFERENCES dbo.Customers(Id), \
             OrderNo varchar(20) NOT NULL, OrderDate datetime NOT NULL, Shipped smalldatetime NULL, Total decimal(12,2) NOT NULL, \
             Status varchar(10) NULL, StartTime time NULL, Placed datetimeoffset NULL, Short char(1) NULL, Tiny varchar(3) NULL)",
        ],
        Dialect::Postgres => [
            "CREATE TABLE public.customers (id serial PRIMARY KEY, code varchar(10) NOT NULL, full_name varchar(100) NOT NULL, \
             email text, phone varchar(20), birth_date date, country char(2), created_at timestamptz, score numeric(5,2), \
             active boolean NOT NULL, note text, avatar bytea, guid uuid, small smallint, rate real, amount money, \
             tags text[], data jsonb, ip inet, calc numeric GENERATED ALWAYS AS (score * 2) STORED, \
             ident integer GENERATED ALWAYS AS IDENTITY)",
            "CREATE TABLE public.orders (id bigserial PRIMARY KEY, customer_id integer NOT NULL REFERENCES public.customers(id), \
             order_no varchar(20) NOT NULL, order_date timestamp NOT NULL, total numeric(12,2) NOT NULL, status varchar(10), \
             start_time time, short char(1), tiny varchar(3), flags bit(3))",
        ],
        Dialect::MySql => [
            "CREATE TABLE customers (id int AUTO_INCREMENT PRIMARY KEY, code varchar(10) NOT NULL, full_name varchar(100) NOT NULL, \
             email varchar(100), phone varchar(20), birth_date date, country char(2), created_at datetime(3), \
             score decimal(5,2), active tinyint(1) NOT NULL, note text, avatar blob, guid char(36), small smallint, \
             tiny tinyint unsigned, stock smallint unsigned NOT NULL, rate float, data json, \
             kind enum('a','b') NOT NULL, flags set('x','y'), y year, \
             bits bit(3), calc decimal(7,2) GENERATED ALWAYS AS (score * 2) STORED)",
            "CREATE TABLE orders (id bigint AUTO_INCREMENT PRIMARY KEY, customer_id int NOT NULL, order_no varchar(20) NOT NULL, \
             order_date datetime NOT NULL, total decimal(12,2) NOT NULL, status varchar(10), start_time time, short char(1), \
             tiny varchar(3), CONSTRAINT fk_orders_customers FOREIGN KEY (customer_id) REFERENCES customers(id))",
        ],
    }
}

async fn count(conn: &mut Connection, d: Dialect, table: &str) -> i64 {
    let sql = format!("SELECT COUNT(*) FROM {}", d.quote_ident(table));
    conn.query_strings(&sql)
        .await
        .ok()
        .and_then(|r| r[0][0].clone())
        .and_then(|v| v.parse().ok())
        .unwrap_or(-1)
}

async fn check(var: &str) {
    let Ok(raw) = std::env::var(var) else {
        eprintln!("{var} not set, skipping");
        return;
    };
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
    for s in tables(d) {
        let out = conn.execute(s, &l).await;
        assert!(!out.failed, "{s}: {:?}", out.messages);
    }
    if d == Dialect::MsSql {
        conn.execute("SET DATEFORMAT dmy", &l).await;
    }
    let snap = load_catalog(&mut conn).await.expect("catalog");
    let fks = snap.catalog.foreign_keys.clone();
    let mut problems = Vec::new();
    let rows = 40;
    for name in ["customers", "orders"] {
        let table = snap
            .catalog
            .objects
            .iter()
            .find(|o| o.kind == ObjectKind::Table && o.name.eq_ignore_ascii_case(name))
            .expect("table")
            .clone();
        let plans = plan_columns(&table, &fks, d);
        let plan_text: Vec<String> = plans
            .iter()
            .map(|p| format!("{} {} → {:?}", p.column, p.data_type, p.generator))
            .collect();
        for seed in 1..=5u64 {
            // what the app reads first: parent rows for foreign keys
            let mut ctx = DataContext::default();
            for f in foreign_keys_of(&table, &fks) {
                let cols = f
                    .to_columns
                    .iter()
                    .map(|c| d.quote_ident(c))
                    .collect::<Vec<_>>()
                    .join(", ");
                let from = format!("{}.{}", d.quote_ident(&f.to_schema), d.quote_ident(&f.to_table));
                let sql = match d {
                    Dialect::MsSql => format!("SELECT DISTINCT TOP (1000) {cols} FROM {from}"),
                    _ => format!("SELECT DISTINCT {cols} FROM {from} LIMIT 1000"),
                };
                ctx.parent_rows
                    .insert(f.name.clone(), conn.query_strings(&sql).await.expect("parents"));
            }
            let opts = TestDataOptions {
                rows,
                null_percent: 30,
                seed,
                ..Default::default()
            };
            let script = match insert_script(&table, &plans, &fks, &ctx, &opts, d) {
                Ok(s) => s,
                Err(err) => {
                    problems.push(format!("{name} seed {seed}: no script: {err}"));
                    continue;
                }
            };
            let before = count(&mut conn, d, &table.name).await;
            let out = conn.execute(&script, &l).await;
            let errors: Vec<&str> = out
                .messages
                .iter()
                .filter(|m| m.kind == MessageKind::Error)
                .map(|m| m.text.as_str())
                .collect();
            let after = count(&mut conn, d, &table.name).await;
            if !errors.is_empty() || after - before != rows as i64 {
                let head: String = script.chars().take(1500).collect();
                problems.push(format!(
                    "{name} seed {seed}: {} rows added, errors {errors:?}\n  plan: {plan_text:?}\n  script: {head}",
                    after - before
                ));
                break;
            }
        }
        // a skipped column is left to its default
        assert!(
            plans.iter().any(|p| p.generator == Generator::Skip),
            "{name}: identity/computed are skipped"
        );
    }
    conn.close().await;
    assert!(
        problems.is_empty(),
        "{var}: {} problem(s):\n{}",
        problems.len(),
        problems.join("\n\n")
    );
}

#[tokio::test]
async fn mssql_generated_data_runs() {
    check("SQLOTUS_TEST_MSSQL").await;
}

#[tokio::test]
async fn postgres_generated_data_runs() {
    check("SQLOTUS_TEST_PG").await;
}

#[tokio::test]
async fn mysql_generated_data_runs() {
    check("SQLOTUS_TEST_MYSQL").await;
}
