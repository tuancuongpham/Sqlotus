//! Schema compare round trips against real servers (skipped unless the
//! SQLOTUS_TEST_* variables are set, see `live.rs`). For each dialect:
//!
//! 1. two databases that differ in every way schema compare looks at;
//! 2. the sync script applied to the target makes it identical to the source;
//! 3. syncing into an empty database creates everything (from scratch);
//! 4. syncing from an empty database drops everything again.
//!
//! Each step re-reads the schema from the server and compares again.

use serde::Deserialize;
use sqlotus_core::Dialect;
use sqlotus_core::schemadiff::{Change, CompareOptions, SchemaModel, compare, sync_script};
use sqlotus_db::{Connection, ConnectionConfig, Limits, MessageKind, execute_script, load_schema};

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

async fn run(conn: &mut Connection, script: &str, what: &str) {
    let out = execute_script(conn, script, &Limits::default()).await;
    let errors: Vec<String> = out
        .iter()
        .flat_map(|b| &b.output.messages)
        .filter(|m| m.kind == MessageKind::Error)
        .map(|m| m.text.clone())
        .collect();
    assert!(
        errors.is_empty(),
        "{what} failed: {errors:#?}\n--- script ---\n{script}"
    );
}

async fn recreate(e: &Env, dialect: Dialect, db: &str) {
    let mut admin = connect(e, None).await;
    let statements: Vec<String> = match dialect {
        Dialect::MsSql => vec![format!(
            "IF DB_ID('{db}') IS NOT NULL BEGIN ALTER DATABASE [{db}] SET SINGLE_USER WITH ROLLBACK IMMEDIATE; DROP DATABASE [{db}]; END; CREATE DATABASE [{db}];"
        )],
        Dialect::Postgres => vec![
            format!("DROP DATABASE IF EXISTS {db} WITH (FORCE)"),
            format!("CREATE DATABASE {db}"),
        ],
        Dialect::MySql => vec![format!("DROP DATABASE IF EXISTS {db}; CREATE DATABASE {db};")],
    };
    for s in statements {
        let out = admin.execute(&s, &Limits::default()).await;
        assert!(!out.failed, "{s}: {:?}", out.messages);
    }
    admin.close().await;
}

async fn schema(e: &Env, db: &str) -> SchemaModel {
    let mut c = connect(e, Some(db)).await;
    let s = load_schema(&mut c).await.expect("load schema");
    c.close().await;
    s
}

fn options(dialect: Dialect) -> CompareOptions {
    CompareOptions {
        case_insensitive_names: dialect != Dialect::Postgres,
        ..Default::default()
    }
}

/// Compare, sync everything into `target`, compare again: must be identical.
async fn sync_all(e: &Env, dialect: Dialect, source: &str, target: &str) -> (usize, String) {
    let opts = options(dialect);
    let src = schema(e, source).await;
    let tgt = schema(e, target).await;
    let diff = compare(&src, &tgt, dialect, &opts);
    let keys: Vec<String> = diff.differences.iter().map(|d| d.key.clone()).collect();
    let script = sync_script(&src, &tgt, &diff, &keys, dialect, &opts);
    let mut conn = connect(e, Some(target)).await;
    run(&mut conn, &script.script, &format!("sync {source} → {target}")).await;
    conn.close().await;
    let after = compare(&src, &schema(e, target).await, dialect, &opts);
    let left: Vec<String> = after
        .differences
        .iter()
        .map(|d| format!("{:?} {} {:?}", d.change, d.key, d.details))
        .collect();
    assert!(
        left.is_empty(),
        "{source} → {target} still differs after sync:\n{left:#?}\n--- script ---\n{}",
        script.script
    );
    (diff.differences.len(), script.script)
}

async fn round_trip(var: &str, prefix: &str, source_ddl: &str, target_ddl: &str, expect: &[(&str, Change)]) {
    let Some(e) = env(var) else {
        eprintln!("{var} not set, skipping");
        return;
    };
    let dialect = e.config.dialect;
    let (src, tgt, empty) = (
        format!("{prefix}_src"),
        format!("{prefix}_tgt"),
        format!("{prefix}_empty"),
    );
    for db in [&src, &tgt, &empty] {
        recreate(&e, dialect, db).await;
    }
    let mut c = connect(&e, Some(&src)).await;
    run(&mut c, source_ddl, "source DDL").await;
    c.close().await;
    let mut c = connect(&e, Some(&tgt)).await;
    run(&mut c, target_ddl, "target DDL").await;
    c.close().await;

    // identical databases compare equal
    let opts = options(dialect);
    let s = schema(&e, &src).await;
    let same = compare(&s, &s, dialect, &opts);
    assert!(same.differences.is_empty());
    assert!(same.identical >= 5, "{}", same.identical);

    // the expected differences are found
    let diff = compare(&s, &schema(&e, &tgt).await, dialect, &opts);
    for (key, change) in expect {
        let found = diff.differences.iter().find(|d| d.key == *key);
        assert_eq!(
            found.map(|d| d.change),
            Some(*change),
            "{key}: {:#?}",
            diff.differences.iter().map(|d| (&d.key, d.change)).collect::<Vec<_>>()
        );
    }
    let customers = diff
        .differences
        .iter()
        .find(|d| d.key.ends_with(".customers"))
        .expect("customers differ");
    assert!(
        customers.warnings.iter().any(|w| w.contains("dropped")),
        "{:?}",
        customers.warnings
    );

    // 2. target becomes the source
    let (n, script) = sync_all(&e, dialect, &src, &tgt).await;
    assert!(n >= expect.len(), "{n}");
    assert!(script.contains("WARNINGS"), "{script}");
    // 3. everything from scratch into an empty database
    sync_all(&e, dialect, &src, &empty).await;
    // 4. and everything dropped again (the target now equals the source)
    recreate(&e, dialect, &format!("{prefix}_none")).await;
    let (dropped, _) = sync_all(&e, dialect, &format!("{prefix}_none"), &tgt).await;
    assert!(dropped >= 5, "{dropped}");
    let left = schema(&e, &tgt).await;
    assert!(left.tables.is_empty() && left.modules.is_empty(), "{left:#?}");
}

const MSSQL_SOURCE: &str = "
CREATE SCHEMA sales;
GO
CREATE TABLE dbo.Customers (Id int IDENTITY(1,1) NOT NULL CONSTRAINT PK_Customers PRIMARY KEY, Name nvarchar(100) NOT NULL,
  Email varchar(200) NULL, Country char(2) NULL CONSTRAINT DF_Customers_Country DEFAULT ('VN'),
  CreatedAt datetime2(3) NOT NULL CONSTRAINT DF_Customers_CreatedAt DEFAULT (sysutcdatetime()), CONSTRAINT UQ_Customers_Email UNIQUE (Email));
CREATE TABLE dbo.Orders (Id int IDENTITY(1,1) NOT NULL CONSTRAINT PK_Orders PRIMARY KEY, CustomerId int NOT NULL, Total decimal(12,2) NOT NULL,
  Status varchar(20) NULL, Amount AS (Total * 2),
  CONSTRAINT FK_Orders_Customers FOREIGN KEY (CustomerId) REFERENCES dbo.Customers(Id) ON DELETE CASCADE, CONSTRAINT CK_Orders_Total CHECK (Total >= 0));
CREATE INDEX IX_Orders_Customer ON dbo.Orders (CustomerId DESC) INCLUDE (Total) WHERE Status IS NOT NULL;
CREATE TABLE dbo.[Order Lines] ([LineNo] int NOT NULL, OrderId int NOT NULL, [User] nvarchar(50) NULL,
  CONSTRAINT PK_OrderLines PRIMARY KEY (OrderId, [LineNo]), CONSTRAINT FK_Lines_Orders FOREIGN KEY (OrderId) REFERENCES dbo.Orders(Id));
CREATE TABLE sales.Regions (Code char(2) NOT NULL CONSTRAINT PK_Regions PRIMARY KEY, Name nvarchar(50) NOT NULL);
CREATE TABLE dbo.Same (Id int NOT NULL PRIMARY KEY, Note nvarchar(10) NULL);
GO
CREATE VIEW dbo.vCustomerOrders AS SELECT c.Name, COUNT(*) AS Orders FROM dbo.Customers c JOIN dbo.Orders o ON o.CustomerId = c.Id GROUP BY c.Name;
GO
CREATE PROCEDURE dbo.pNew @Id int AS SELECT Id, Total FROM dbo.Orders WHERE Id = @Id;
GO
CREATE FUNCTION dbo.fTotal(@CustomerId int) RETURNS decimal(12,2) AS BEGIN RETURN (SELECT SUM(Total) FROM dbo.Orders WHERE CustomerId = @CustomerId) END;
GO
CREATE TRIGGER dbo.tOrders ON dbo.Orders AFTER INSERT AS SET NOCOUNT ON;
GO
CREATE VIEW sales.vRegions AS SELECT Code FROM sales.Regions;
";

const MSSQL_TARGET: &str = "
CREATE TABLE dbo.Customers (Id int IDENTITY(1,1) NOT NULL CONSTRAINT PK_Customers PRIMARY KEY, Name nvarchar(200) NULL,
  Country char(2) NULL CONSTRAINT DF_Customers_Country DEFAULT ('US'),
  CreatedAt datetime2(3) NOT NULL CONSTRAINT DF_Customers_CreatedAt DEFAULT (sysutcdatetime()), Legacy int NULL CONSTRAINT DF_Customers_Legacy DEFAULT (0));
CREATE TABLE dbo.Orders (Id int IDENTITY(1,1) NOT NULL CONSTRAINT PK_Orders PRIMARY KEY, CustomerId int NOT NULL, Total decimal(12,2) NOT NULL,
  Amount AS (Total * 2),
  CONSTRAINT FK_Orders_Customers FOREIGN KEY (CustomerId) REFERENCES dbo.Customers(Id), CONSTRAINT CK_Orders_Total CHECK (Total > 0));
CREATE INDEX IX_Orders_Old ON dbo.Orders (Total);
CREATE TABLE dbo.Obsolete (Id int NOT NULL CONSTRAINT PK_Obsolete PRIMARY KEY, CustomerId int NULL CONSTRAINT FK_Obsolete_Customers REFERENCES dbo.Customers(Id));
CREATE TABLE dbo.Same (Id int NOT NULL PRIMARY KEY, Note nvarchar(10) NULL);
GO
CREATE VIEW dbo.vCustomerOrders AS SELECT c.Name, 0 AS Orders FROM dbo.Customers c;
GO
CREATE PROCEDURE dbo.pOld AS SELECT 1 AS x;
GO
CREATE TRIGGER dbo.tOrders ON dbo.Orders AFTER INSERT AS SET NOCOUNT ON;
";

#[tokio::test]
async fn mssql_schema_compare_round_trip() {
    round_trip(
        "SQLOTUS_TEST_MSSQL",
        "SqlotusCmp",
        MSSQL_SOURCE,
        MSSQL_TARGET,
        &[
            ("table:dbo.customers", Change::Changed),
            ("table:dbo.orders", Change::Changed),
            ("table:dbo.order lines", Change::Added),
            ("table:sales.regions", Change::Added),
            ("table:dbo.obsolete", Change::Removed),
            ("view:dbo.vcustomerorders", Change::Changed),
            ("procedure:dbo.pnew", Change::Added),
            ("function:dbo.ftotal", Change::Added),
            ("procedure:dbo.pold", Change::Removed),
            ("view:sales.vregions", Change::Added),
        ],
    )
    .await;
}

const PG_SOURCE: &str = "
CREATE SCHEMA sales;
CREATE TABLE public.customers (id integer GENERATED BY DEFAULT AS IDENTITY PRIMARY KEY, name varchar(100) NOT NULL, email varchar(200),
  country char(2) DEFAULT 'VN', created_at timestamptz NOT NULL DEFAULT now(), CONSTRAINT uq_customers_email UNIQUE (email));
CREATE TABLE public.orders (id serial PRIMARY KEY, customer_id integer NOT NULL, total numeric(12,2) NOT NULL, status varchar(20),
  amount numeric GENERATED ALWAYS AS (total * 2) STORED,
  CONSTRAINT fk_orders_customers FOREIGN KEY (customer_id) REFERENCES public.customers(id) ON DELETE CASCADE,
  CONSTRAINT ck_orders_total CHECK (total >= 0));
CREATE INDEX ix_orders_customer ON public.orders (customer_id DESC) INCLUDE (total) WHERE status IS NOT NULL;
CREATE INDEX ix_customers_lower_name ON public.customers (lower(name));
CREATE TABLE public.\"Order Lines\" (\"user\" varchar(50), line_no integer NOT NULL, order_id integer NOT NULL,
  CONSTRAINT pk_order_lines PRIMARY KEY (order_id, line_no), CONSTRAINT fk_lines_orders FOREIGN KEY (order_id) REFERENCES public.orders(id));
CREATE TABLE sales.regions (code char(2) PRIMARY KEY, name varchar(50) NOT NULL);
CREATE TABLE public.same (id integer PRIMARY KEY, note varchar(10));
CREATE VIEW public.v_customer_orders AS SELECT c.name, count(*) AS orders FROM public.customers c JOIN public.orders o ON o.customer_id = c.id GROUP BY c.name;
CREATE FUNCTION public.f_total(p_customer integer) RETURNS numeric LANGUAGE sql AS $$ SELECT sum(total) FROM public.orders WHERE customer_id = p_customer $$;
CREATE PROCEDURE public.p_new(p_id integer) LANGUAGE plpgsql AS $$ BEGIN PERFORM p_id; END $$;
CREATE FUNCTION public.trg_orders() RETURNS trigger LANGUAGE plpgsql AS $$ BEGIN RETURN NEW; END $$;
CREATE TRIGGER t_orders AFTER INSERT ON public.orders FOR EACH ROW EXECUTE FUNCTION public.trg_orders();
CREATE VIEW sales.v_regions AS SELECT code FROM sales.regions;
";

const PG_TARGET: &str = "
CREATE TABLE public.customers (id integer GENERATED BY DEFAULT AS IDENTITY PRIMARY KEY, name varchar(200), country char(2) DEFAULT 'US',
  created_at timestamptz NOT NULL DEFAULT now(), legacy integer DEFAULT 0);
CREATE TABLE public.orders (id serial PRIMARY KEY, customer_id integer NOT NULL, total numeric(12,2) NOT NULL,
  amount numeric GENERATED ALWAYS AS (total * 2) STORED,
  CONSTRAINT fk_orders_customers FOREIGN KEY (customer_id) REFERENCES public.customers(id), CONSTRAINT ck_orders_total CHECK (total > 0));
CREATE INDEX ix_orders_old ON public.orders (total);
CREATE TABLE public.obsolete (id integer PRIMARY KEY, customer_id integer REFERENCES public.customers(id));
CREATE TABLE public.same (id integer PRIMARY KEY, note varchar(10));
CREATE VIEW public.v_customer_orders AS SELECT c.name, 0 AS orders FROM public.customers c;
CREATE FUNCTION public.f_old() RETURNS integer LANGUAGE sql AS $$ SELECT 1 $$;
CREATE FUNCTION public.trg_orders() RETURNS trigger LANGUAGE plpgsql AS $$ BEGIN RETURN NEW; END $$;
CREATE TRIGGER t_orders AFTER INSERT ON public.orders FOR EACH ROW EXECUTE FUNCTION public.trg_orders();
";

#[tokio::test]
async fn postgres_schema_compare_round_trip() {
    round_trip(
        "SQLOTUS_TEST_PG",
        "sqlotus_cmp",
        PG_SOURCE,
        PG_TARGET,
        &[
            ("table:public.customers", Change::Changed),
            ("table:public.orders", Change::Changed),
            ("table:public.order lines", Change::Added),
            ("table:sales.regions", Change::Added),
            ("table:public.obsolete", Change::Removed),
            ("view:public.v_customer_orders", Change::Changed),
            ("function:public.f_total", Change::Added),
            ("procedure:public.p_new", Change::Added),
            ("function:public.f_old", Change::Removed),
        ],
    )
    .await;
}

const MYSQL_SOURCE: &str = "
CREATE TABLE Customers (Id int NOT NULL AUTO_INCREMENT, Name varchar(100) NOT NULL, Email varchar(200) NULL, Country char(2) NULL DEFAULT 'VN',
  CreatedAt datetime NOT NULL DEFAULT CURRENT_TIMESTAMP, PRIMARY KEY (Id), UNIQUE KEY UQ_Customers_Email (Email));
CREATE TABLE Orders (Id int NOT NULL AUTO_INCREMENT, CustomerId int NOT NULL, Total decimal(12,2) NOT NULL, Status varchar(20) NULL,
  Amount decimal(14,2) GENERATED ALWAYS AS (Total * 2) VIRTUAL,
  PRIMARY KEY (Id), KEY IX_Orders_Customer (CustomerId DESC, Total),
  CONSTRAINT FK_Orders_Customers FOREIGN KEY (CustomerId) REFERENCES Customers (Id) ON DELETE CASCADE, CONSTRAINT CK_Orders_Total CHECK (Total >= 0));
CREATE TABLE `Order Lines` (`LineNo` int NOT NULL, OrderId int NOT NULL, `User` varchar(50) NULL, PRIMARY KEY (OrderId, `LineNo`),
  CONSTRAINT FK_Lines_Orders FOREIGN KEY (OrderId) REFERENCES Orders (Id));
CREATE TABLE Same (Id int NOT NULL, Note varchar(10) NULL, PRIMARY KEY (Id));
CREATE VIEW vCustomerOrders AS SELECT c.Name, COUNT(*) AS Orders FROM Customers c JOIN Orders o ON o.CustomerId = c.Id GROUP BY c.Name;
CREATE PROCEDURE pNew(IN p_id INT) BEGIN SELECT Id, Total FROM Orders WHERE Id = p_id; END;
CREATE FUNCTION fTotal(p_customer INT) RETURNS DECIMAL(12,2) READS SQL DATA RETURN (SELECT SUM(Total) FROM Orders WHERE CustomerId = p_customer);
CREATE TRIGGER tOrders AFTER INSERT ON Orders FOR EACH ROW SET @last_order = NEW.Id;
";

const MYSQL_TARGET: &str = "
CREATE TABLE Customers (Id int NOT NULL AUTO_INCREMENT, Name varchar(200) NULL, Country char(2) NULL DEFAULT 'US',
  CreatedAt datetime NOT NULL DEFAULT CURRENT_TIMESTAMP, Legacy int NULL DEFAULT 0, PRIMARY KEY (Id));
CREATE TABLE Orders (Id int NOT NULL AUTO_INCREMENT, CustomerId int NOT NULL, Total decimal(12,2) NOT NULL,
  Amount decimal(14,2) GENERATED ALWAYS AS (Total * 2) VIRTUAL,
  PRIMARY KEY (Id), KEY IX_Orders_Old (Total),
  CONSTRAINT FK_Orders_Customers FOREIGN KEY (CustomerId) REFERENCES Customers (Id), CONSTRAINT CK_Orders_Total CHECK (Total > 0));
CREATE TABLE Obsolete (Id int NOT NULL, CustomerId int NULL, PRIMARY KEY (Id), CONSTRAINT FK_Obsolete_Customers FOREIGN KEY (CustomerId) REFERENCES Customers (Id));
CREATE TABLE Same (Id int NOT NULL, Note varchar(10) NULL, PRIMARY KEY (Id));
CREATE VIEW vCustomerOrders AS SELECT c.Name, 0 AS Orders FROM Customers c;
CREATE PROCEDURE pOld() SELECT 1;
CREATE TRIGGER tOrders AFTER INSERT ON Orders FOR EACH ROW SET @last_order = NEW.Id;
";

#[tokio::test]
async fn mysql_schema_compare_round_trip() {
    round_trip(
        "SQLOTUS_TEST_MYSQL",
        "sqlotus_cmp",
        MYSQL_SOURCE,
        MYSQL_TARGET,
        &[
            ("table:.customers", Change::Changed),
            ("table:.orders", Change::Changed),
            ("table:.order lines", Change::Added),
            ("table:.obsolete", Change::Removed),
            ("view:.vcustomerorders", Change::Changed),
            ("procedure:.pnew", Change::Added),
            ("function:.ftotal", Change::Added),
            ("procedure:.pold", Change::Removed),
        ],
    )
    .await;
}

/// A login that may read data but not definitions: hidden objects must be
/// reported as skipped, never as missing (which would drop them).
#[tokio::test]
async fn mssql_login_without_view_definition() {
    let Some(e) = env("SQLOTUS_TEST_MSSQL") else {
        return;
    };
    let db = "SqlotusCmpPerm";
    recreate(&e, Dialect::MsSql, db).await;
    let mut c = connect(&e, Some(db)).await;
    run(&mut c, MSSQL_SOURCE, "source DDL").await;
    let password = format!("Rd!{}x9", std::process::id());
    run(
        &mut c,
        &format!(
            "IF SUSER_ID('sqlotus_reader') IS NULL CREATE LOGIN sqlotus_reader WITH PASSWORD = '{password}', CHECK_POLICY = OFF;\n\
             ELSE ALTER LOGIN sqlotus_reader WITH PASSWORD = '{password}';\n\
             CREATE USER sqlotus_reader FOR LOGIN sqlotus_reader;\nALTER ROLE db_datareader ADD MEMBER sqlotus_reader;"
        ),
        "reader login",
    )
    .await;
    c.close().await;
    let admin_view = schema(&e, db).await;
    let mut cfg = e.config.clone();
    cfg.database = Some(db.into());
    cfg.user = Some("sqlotus_reader".into());
    let mut reader = Connection::open(&cfg, Some(&password)).await.expect("reader connects");
    let refused = load_schema(&mut reader)
        .await
        .expect_err("must refuse without VIEW DEFINITION")
        .to_string();
    assert!(
        refused.contains("VIEW DEFINITION") && refused.contains("would drop them"),
        "{refused}"
    );
    reader.close().await;
    // with the permission the reader sees everything the owner sees
    let mut c = connect(&e, Some(db)).await;
    run(&mut c, "GRANT VIEW DEFINITION TO sqlotus_reader;", "grant").await;
    c.close().await;
    let mut reader = Connection::open(&cfg, Some(&password)).await.expect("reader connects");
    let reader_view = load_schema(&mut reader).await.expect("reader loads");
    reader.close().await;
    eprintln!(
        "reader sees {} tables, {} modules, unreadable {:?}",
        reader_view.tables.len(),
        reader_view.modules.len(),
        reader_view.unreadable
    );
    assert!(reader_view.unreadable.is_empty(), "{:?}", reader_view.unreadable);
    for (src, tgt) in [(&reader_view, &admin_view), (&admin_view, &reader_view)] {
        let d = compare(src, tgt, Dialect::MsSql, &options(Dialect::MsSql));
        let wrong: Vec<String> = d
            .differences
            .iter()
            .map(|x| format!("{:?} {} {:?}", x.change, x.key, x.details))
            .collect();
        assert!(wrong.is_empty(), "the reader must see the same schema: {wrong:#?}");
    }
    let mut admin = connect(&e, None).await;
    let _ = admin.execute("ALTER DATABASE [SqlotusCmpPerm] SET SINGLE_USER WITH ROLLBACK IMMEDIATE; DROP DATABASE [SqlotusCmpPerm]; DROP LOGIN sqlotus_reader;", &Limits::default()).await;
}
