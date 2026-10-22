//! Schema compare on awkward schemas against real servers (skipped unless
//! the SQLOTUS_TEST_* variables are set, see `live.rs`): names with spaces,
//! Unicode and reserved words, computed columns, expression defaults,
//! CHECK and UNIQUE constraints, filtered and covering indexes, composite
//! and self-referencing foreign keys with actions, views on views,
//! sequences, synonyms, tables of one name in two schemas whose foreign
//! keys share a name too. Two different databases are synced both ways, into
//! an empty one and back to nothing; each script must run, the databases
//! must then compare equal, and rows of the target must survive any change
//! that does not announce data loss.

use serde::Deserialize;
use sqlotus_core::Dialect;
use sqlotus_core::schemadiff::{CompareOptions, SchemaModel, compare, sync_script};
use sqlotus_core::schemafolder::Snapshot;
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

async fn run(conn: &mut Connection, script: &str) -> Vec<String> {
    execute_script(conn, script, &Limits::default())
        .await
        .iter()
        .flat_map(|b| &b.output.messages)
        .filter(|m| m.kind == MessageKind::Error)
        .map(|m| m.text.clone())
        .collect()
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

/// Sync `source` into `target`; problems found, if any.
async fn sync(e: &Env, d: Dialect, source: &str, target: &str, problems: &mut Vec<String>) -> String {
    let opts = options(d);
    let src = schema(e, source).await;
    let tgt = schema(e, target).await;
    let diff = compare(&src, &tgt, d, &opts);
    let keys: Vec<String> = diff.differences.iter().map(|x| x.key.clone()).collect();
    let script = sync_script(&src, &tgt, &diff, &keys, d, &opts);
    let mut conn = connect(e, Some(target)).await;
    let errors = run(&mut conn, &script.script).await;
    conn.close().await;
    if !errors.is_empty() {
        problems.push(format!(
            "{source} → {target}: the script failed: {errors:#?}\n--- script ---\n{}",
            script.script
        ));
        return script.script;
    }
    let after = compare(&src, &schema(e, target).await, d, &opts);
    if !after.differences.is_empty() {
        problems.push(format!(
            "{source} → {target}: still different after the sync: {:#?}\n--- script ---\n{}",
            after
                .differences
                .iter()
                .map(|x| format!("{:?} {} {:?}", x.change, x.key, x.details))
                .collect::<Vec<_>>(),
            script.script
        ));
    }
    script.script
}

struct Case {
    a: &'static str,
    b: &'static str,
    /// Rows put into B before syncing A into it, and the query that reads
    /// them back afterwards (columns both versions share).
    seed: &'static str,
    check: &'static str,
    /// The table whose rows are checked, as named in data-loss warnings.
    table: &'static str,
}

const MSSQL: Case = Case {
    a: "CREATE SCHEMA [sales ops];
GO
CREATE SEQUENCE dbo.OrderNo AS int START WITH 1000 INCREMENT BY 1;
CREATE TABLE dbo.[Customer List] (Id int IDENTITY(1,1) NOT NULL CONSTRAINT PK_CustomerList PRIMARY KEY,
  [Name] nvarchar(100) NOT NULL, Email varchar(200) NULL CONSTRAINT UQ_CustomerList_Email UNIQUE, [Order] int NULL,
  Score decimal(9,2) NOT NULL CONSTRAINT DF_CustomerList_Score DEFAULT (0),
  Created datetime2(3) NOT NULL CONSTRAINT DF_CustomerList_Created DEFAULT (sysutcdatetime()),
  Code AS (upper(left([Name], (3)))) PERSISTED,
  ParentId int NULL CONSTRAINT FK_CustomerList_Parent REFERENCES dbo.[Customer List](Id), [Tên] nvarchar(50) NULL,
  CONSTRAINT CK_CustomerList_Score CHECK (Score >= (0)));
CREATE INDEX IX_CustomerList_Email ON dbo.[Customer List] (Email) WHERE Email IS NOT NULL;
CREATE UNIQUE INDEX UX_CustomerList_Name ON dbo.[Customer List] ([Name], Id) INCLUDE (Score);
CREATE TABLE [sales ops].Orders (OrderId int NOT NULL, [LineNo] smallint NOT NULL, CustomerId int NOT NULL,
  Amount money NOT NULL, Note nvarchar(max) NULL, CONSTRAINT PK_Orders PRIMARY KEY (OrderId, [LineNo]),
  CONSTRAINT FK_Orders_Customer FOREIGN KEY (CustomerId) REFERENCES dbo.[Customer List](Id) ON DELETE CASCADE);
CREATE TABLE [sales ops].Shipments (ShipId int NOT NULL CONSTRAINT PK_Shipments PRIMARY KEY, OrderId int NOT NULL,
  [LineNo] smallint NOT NULL, CONSTRAINT FK_Ship_Order FOREIGN KEY (OrderId, [LineNo])
  REFERENCES [sales ops].Orders (OrderId, [LineNo]) ON UPDATE CASCADE);
GO
CREATE VIEW dbo.vBase AS SELECT Id, [Name] FROM dbo.[Customer List];
GO
CREATE VIEW dbo.vTop AS SELECT b.Id, b.[Name], o.Amount FROM dbo.vBase b JOIN [sales ops].Orders o ON o.CustomerId = b.Id;
GO
CREATE FUNCTION dbo.fScore (@Id int) RETURNS decimal(9,2) AS BEGIN RETURN (SELECT Score FROM dbo.[Customer List] WHERE Id = @Id) END;
GO
CREATE PROCEDURE [sales ops].[Get Orders] @CustomerId int AS SELECT OrderId FROM [sales ops].Orders WHERE CustomerId = @CustomerId;
GO
CREATE TRIGGER dbo.tCustomerList ON dbo.[Customer List] AFTER UPDATE AS SET NOCOUNT ON;
GO
CREATE SYNONYM dbo.Clients FOR dbo.[Customer List];
CREATE TABLE dbo.Kept (Id int NOT NULL CONSTRAINT PK_Kept PRIMARY KEY, Amount decimal(9,2) NOT NULL,
  Twice AS (Amount * 2), CONSTRAINT CK_Kept_Amount CHECK (Amount >= 0));
CREATE INDEX IX_Kept_Amount ON dbo.Kept (Amount);
CREATE TABLE dbo.MirrorTarget (Id int NOT NULL CONSTRAINT PK_MirrorTarget PRIMARY KEY);
CREATE TABLE dbo.Mirror (Id int NOT NULL CONSTRAINT PK_dbo_Mirror PRIMARY KEY,
  TargetId int NULL CONSTRAINT FK_Mirror_Target REFERENCES dbo.MirrorTarget (Id));
CREATE TABLE [sales ops].Mirror (Id int NOT NULL CONSTRAINT PK_sales_Mirror PRIMARY KEY,
  TargetId int NULL CONSTRAINT FK_Mirror_Target REFERENCES dbo.MirrorTarget (Id));",
    b: "CREATE SCHEMA [sales ops];
GO
CREATE SEQUENCE dbo.OrderNo AS int START WITH 1 INCREMENT BY 5;
CREATE TABLE dbo.[Customer List] (Id int IDENTITY(1,1) NOT NULL CONSTRAINT PK_CustomerList PRIMARY KEY,
  [Name] nvarchar(50) NOT NULL, Email varchar(100) NULL, [Order] int NULL,
  Score decimal(9,2) NULL CONSTRAINT DF_CustomerList_Score DEFAULT (1), Legacy int NULL);
CREATE INDEX IX_CustomerList_Name ON dbo.[Customer List] ([Name]);
CREATE TABLE [sales ops].Orders (OrderId int NOT NULL, [LineNo] smallint NOT NULL, CustomerId int NOT NULL,
  Amount decimal(12,2) NOT NULL, CONSTRAINT PK_Orders PRIMARY KEY (OrderId, [LineNo]),
  CONSTRAINT FK_Orders_Customer FOREIGN KEY (CustomerId) REFERENCES dbo.[Customer List](Id));
CREATE TABLE dbo.Obsolete (Id int NOT NULL PRIMARY KEY, CustomerId int NULL REFERENCES dbo.[Customer List](Id));
GO
CREATE VIEW dbo.vBase AS SELECT Id FROM dbo.[Customer List];
GO
CREATE FUNCTION dbo.fScore (@Id int) RETURNS decimal(9,2) AS BEGIN RETURN 0 END;
GO
CREATE PROCEDURE dbo.pOld AS SELECT 1 AS x;
GO
CREATE SYNONYM dbo.Clients FOR [sales ops].Orders;
CREATE TABLE dbo.Kept (Id int NOT NULL CONSTRAINT PK_Kept PRIMARY KEY, Amount decimal(12,2) NULL,
  Twice AS (Amount * 2), CONSTRAINT CK_Kept_Amount CHECK (Amount >= 0));
CREATE INDEX IX_Kept_Amount ON dbo.Kept (Amount);",
    seed: "SET IDENTITY_INSERT dbo.[Customer List] ON;
INSERT INTO dbo.[Customer List] (Id, [Name], Email, [Order], Score) VALUES (1, N'Nguyễn', 'a@example.com', 3, 1.5), (2, N'Trần', NULL, NULL, 2);
SET IDENTITY_INSERT dbo.[Customer List] OFF;
INSERT INTO [sales ops].Orders (OrderId, [LineNo], CustomerId, Amount) VALUES (10, 1, 1, 12.5);
INSERT INTO dbo.Kept (Id, Amount) VALUES (1, 7.25);",
    check: "SELECT CAST(Id AS varchar(10)) + ':' + [Name] + ':' + ISNULL(Email, '-') + ':' + CAST(Score AS varchar(20)) FROM dbo.[Customer List] UNION ALL SELECT 'kept ' + CAST(Amount AS varchar(20)) + ' ' + CAST(Twice AS varchar(20)) FROM dbo.Kept ORDER BY 1",
    table: "Customer List",
};

const POSTGRES: Case = Case {
    a: "CREATE SCHEMA \"sales ops\";
CREATE SEQUENCE public.order_no START 1000 INCREMENT 1;
CREATE TABLE public.\"Customer List\" (id integer GENERATED BY DEFAULT AS IDENTITY PRIMARY KEY,
  name varchar(100) NOT NULL, email varchar(200) UNIQUE, \"order\" integer,
  score numeric(9,2) NOT NULL DEFAULT 0 CHECK (score >= 0), created timestamptz NOT NULL DEFAULT now(),
  code text GENERATED ALWAYS AS (upper(left(name, 3))) STORED,
  parent_id integer REFERENCES public.\"Customer List\"(id), \"Tên\" varchar(50));
CREATE INDEX ix_cl_email ON public.\"Customer List\" (email) WHERE email IS NOT NULL;
CREATE UNIQUE INDEX ux_cl_name ON public.\"Customer List\" (name, id) INCLUDE (score);
CREATE TABLE \"sales ops\".orders (order_id integer NOT NULL, line_no smallint NOT NULL,
  customer_id integer NOT NULL REFERENCES public.\"Customer List\"(id) ON DELETE CASCADE,
  amount numeric(12,2) NOT NULL, note text, PRIMARY KEY (order_id, line_no));
CREATE TABLE \"sales ops\".shipments (ship_id integer PRIMARY KEY, order_id integer NOT NULL, line_no smallint NOT NULL,
  FOREIGN KEY (order_id, line_no) REFERENCES \"sales ops\".orders (order_id, line_no) ON UPDATE CASCADE);
CREATE VIEW public.v_base AS SELECT id, name FROM public.\"Customer List\";
CREATE VIEW public.v_top AS SELECT b.id, b.name, o.amount FROM public.v_base b JOIN \"sales ops\".orders o ON o.customer_id = b.id;
CREATE FUNCTION public.f_score(p_id integer) RETURNS numeric LANGUAGE sql AS $$ SELECT score FROM public.\"Customer List\" WHERE id = p_id $$;
CREATE PROCEDURE \"sales ops\".\"get orders\"(p_customer integer) LANGUAGE plpgsql AS $$ BEGIN PERFORM 1; END $$;
CREATE FUNCTION public.trg() RETURNS trigger LANGUAGE plpgsql AS $$ BEGIN RETURN NEW; END $$;
CREATE TRIGGER t_cl BEFORE UPDATE ON public.\"Customer List\" FOR EACH ROW EXECUTE FUNCTION public.trg();
CREATE TABLE public.kept (id integer PRIMARY KEY, amount numeric(9,2) NOT NULL,
  twice numeric GENERATED ALWAYS AS (amount * 2) STORED, CONSTRAINT kept_amount CHECK (amount >= 0));
CREATE INDEX ix_kept_amount ON public.kept (amount);
CREATE VIEW public.v_kept AS SELECT id, amount FROM public.kept;
CREATE VIEW public.v_kept2 AS SELECT id FROM public.v_kept;
CREATE TABLE public.mirror_target (id integer PRIMARY KEY);
CREATE TABLE public.mirror (id integer PRIMARY KEY,
  target_id integer CONSTRAINT fk_mirror_target REFERENCES public.mirror_target (id));
CREATE TABLE \"sales ops\".mirror (id integer PRIMARY KEY,
  target_id integer CONSTRAINT fk_mirror_target REFERENCES public.mirror_target (id));",
    b: "CREATE SCHEMA \"sales ops\";
CREATE SEQUENCE public.order_no START 1 INCREMENT 5;
CREATE TABLE public.\"Customer List\" (id integer GENERATED BY DEFAULT AS IDENTITY PRIMARY KEY,
  name varchar(50) NOT NULL, email varchar(100), \"order\" integer, score numeric(9,2) DEFAULT 1, legacy integer);
CREATE INDEX ix_cl_name ON public.\"Customer List\" (name);
CREATE TABLE \"sales ops\".orders (order_id integer NOT NULL, line_no smallint NOT NULL,
  customer_id integer NOT NULL REFERENCES public.\"Customer List\"(id), amount numeric(10,2) NOT NULL,
  PRIMARY KEY (order_id, line_no));
CREATE TABLE public.obsolete (id integer PRIMARY KEY, customer_id integer REFERENCES public.\"Customer List\"(id));
CREATE VIEW public.v_base AS SELECT id FROM public.\"Customer List\";
CREATE FUNCTION public.f_score(p_id integer) RETURNS numeric LANGUAGE sql AS $$ SELECT 0::numeric $$;
CREATE FUNCTION public.f_old() RETURNS integer LANGUAGE sql AS $$ SELECT 1 $$;
CREATE TABLE public.kept (id integer PRIMARY KEY, amount numeric(12,2),
  twice numeric GENERATED ALWAYS AS (amount * 2) STORED, CONSTRAINT kept_amount CHECK (amount >= 0));
CREATE INDEX ix_kept_amount ON public.kept (amount);
CREATE VIEW public.v_kept AS SELECT id, amount FROM public.kept;
CREATE VIEW public.v_kept2 AS SELECT id FROM public.v_kept;",
    seed: "INSERT INTO public.\"Customer List\" (id, name, email, \"order\", score) VALUES (1, 'Nguyễn', 'a@example.com', 3, 1.5), (2, 'Trần', NULL, NULL, 2);
INSERT INTO \"sales ops\".orders (order_id, line_no, customer_id, amount) VALUES (10, 1, 1, 12.5);
INSERT INTO public.kept (id, amount) VALUES (1, 7.25);",
    check: "SELECT id || ':' || name || ':' || coalesce(email, '-') || ':' || score FROM public.\"Customer List\" UNION ALL SELECT 'kept ' || amount || ' ' || twice FROM public.kept ORDER BY 1",
    table: "Customer List",
};

const MYSQL: Case = Case {
    a: "CREATE TABLE `customer list` (id int AUTO_INCREMENT PRIMARY KEY, name varchar(100) NOT NULL,
  email varchar(200) UNIQUE, `order` int, score decimal(9,2) NOT NULL DEFAULT 0,
  created datetime(3) NOT NULL DEFAULT CURRENT_TIMESTAMP(3),
  code varchar(3) GENERATED ALWAYS AS (upper(left(name, 3))) STORED, parent_id int, `tên` varchar(50),
  status enum('a','b') NOT NULL DEFAULT 'a', flags set('x','y'), amount decimal(12,2) unsigned,
  CONSTRAINT ck_score CHECK (score >= 0), CONSTRAINT fk_parent FOREIGN KEY (parent_id) REFERENCES `customer list`(id),
  KEY ix_name (name, id), FULLTEXT KEY ft_name (name)) ENGINE=InnoDB DEFAULT CHARSET=utf8mb4;
CREATE TABLE orders (order_id int NOT NULL, line_no smallint NOT NULL, customer_id int NOT NULL,
  amount decimal(12,2) NOT NULL, note text, PRIMARY KEY (order_id, line_no),
  CONSTRAINT fk_orders_customer FOREIGN KEY (customer_id) REFERENCES `customer list`(id) ON DELETE CASCADE);
CREATE TABLE shipments (ship_id int PRIMARY KEY, order_id int NOT NULL, line_no smallint NOT NULL,
  CONSTRAINT fk_ship_order FOREIGN KEY (order_id, line_no) REFERENCES orders (order_id, line_no) ON UPDATE CASCADE);
CREATE VIEW v_base AS SELECT id, name FROM `customer list`;
CREATE VIEW v_top AS SELECT b.id, b.name, o.amount FROM v_base b JOIN orders o ON o.customer_id = b.id;
CREATE FUNCTION f_score(p_id int) RETURNS decimal(9,2) DETERMINISTIC READS SQL DATA RETURN (SELECT score FROM `customer list` WHERE id = p_id);
CREATE PROCEDURE `get orders`(IN p_customer int) SELECT order_id FROM orders WHERE customer_id = p_customer;
CREATE TRIGGER t_cl BEFORE UPDATE ON `customer list` FOR EACH ROW SET NEW.score = NEW.score;
CREATE TABLE kept (id int PRIMARY KEY, amount decimal(9,2) NOT NULL, twice decimal(14,2) GENERATED ALWAYS AS (amount * 2) STORED,
  CONSTRAINT kept_amount CHECK (amount >= 0), KEY ix_kept_amount (amount));",
    b: "CREATE TABLE `customer list` (id int AUTO_INCREMENT PRIMARY KEY, name varchar(50) NOT NULL, email varchar(100),
  `order` int, score decimal(9,2) DEFAULT 1, legacy int, KEY ix_cl_name (name)) ENGINE=InnoDB DEFAULT CHARSET=utf8mb4;
CREATE TABLE orders (order_id int NOT NULL, line_no smallint NOT NULL, customer_id int NOT NULL,
  amount decimal(10,2) NOT NULL, PRIMARY KEY (order_id, line_no),
  CONSTRAINT fk_orders_customer FOREIGN KEY (customer_id) REFERENCES `customer list`(id));
CREATE TABLE obsolete (id int PRIMARY KEY, customer_id int, CONSTRAINT fk_obsolete FOREIGN KEY (customer_id) REFERENCES `customer list`(id));
CREATE VIEW v_base AS SELECT id FROM `customer list`;
CREATE FUNCTION f_score(p_id int) RETURNS decimal(9,2) DETERMINISTIC RETURN 0;
CREATE PROCEDURE p_old() SELECT 1 AS x;
CREATE TABLE kept (id int PRIMARY KEY, amount decimal(12,2), twice decimal(14,2) GENERATED ALWAYS AS (amount * 2) STORED,
  CONSTRAINT kept_amount CHECK (amount >= 0), KEY ix_kept_amount (amount));",
    seed: "INSERT INTO `customer list` (id, name, email, `order`, score) VALUES (1, 'Nguyễn', 'a@example.com', 3, 1.5), (2, 'Trần', NULL, NULL, 2);
INSERT INTO orders (order_id, line_no, customer_id, amount) VALUES (10, 1, 1, 12.5);
INSERT INTO kept (id, amount) VALUES (1, 7.25);",
    check: "SELECT CONCAT(id, ':', name, ':', COALESCE(email, '-'), ':', score) FROM `customer list` UNION ALL SELECT CONCAT('kept ', amount, ' ', twice) FROM kept ORDER BY 1",
    table: "customer list",
};

async fn torture(var: &str, case: &Case) {
    let Some(e) = env(var) else {
        eprintln!("{var} not set, skipping");
        return;
    };
    let d = e.config.dialect;
    let (a, b, empty, none) = ("torture_a", "torture_b", "torture_empty", "torture_none");
    for db in [a, b, empty, none] {
        recreate(&e, d, db).await;
    }
    let mut problems = Vec::new();
    for (db, ddl) in [(a, case.a), (b, case.b)] {
        let mut c = connect(&e, Some(db)).await;
        let errors = run(&mut c, ddl).await;
        assert!(errors.is_empty(), "{db}: {errors:#?}");
        c.close().await;
    }
    let mut c = connect(&e, Some(b)).await;
    let errors = run(&mut c, case.seed).await;
    assert!(errors.is_empty(), "seed: {errors:#?}");
    let rows_before = c.query_strings(case.check).await.expect("rows");
    c.close().await;

    // B becomes A; rows survive unless the script says they may not
    let script = sync(&e, d, a, b, &mut problems).await;
    let mut c = connect(&e, Some(b)).await;
    let rows_after = c.query_strings(case.check).await.unwrap_or_default();
    c.close().await;
    let loss_announced = script
        .lines()
        .any(|l| l.contains("WARNING") && l.contains(case.table) && l.contains("data"));
    if rows_after != rows_before && !loss_announced {
        problems.push(format!(
            "rows of {} changed without a warning: {rows_before:?} → {rows_after:?}\n--- script ---\n{script}",
            case.table
        ));
    }
    // ... and back: A becomes the old B (recreated)
    recreate(&e, d, b).await;
    let mut c = connect(&e, Some(b)).await;
    run(&mut c, case.b).await;
    c.close().await;
    sync(&e, d, b, a, &mut problems).await;
    // from scratch, and back to nothing
    recreate(&e, d, a).await;
    let mut c = connect(&e, Some(a)).await;
    run(&mut c, case.a).await;
    c.close().await;
    sync(&e, d, a, empty, &mut problems).await;
    // source control: the snapshot a folder keeps reads back as the same
    // schema, and "get latest" from it rebuilds the database
    let model = schema(&e, a).await;
    let back = Snapshot::from_json(&Snapshot::new(d, model.clone()).to_json())
        .expect("snapshot")
        .model;
    let opts = options(d);
    let lost = compare(&model, &back, d, &opts);
    if !lost.differences.is_empty() {
        problems.push(format!(
            "the folder snapshot differs from the database: {:#?}",
            lost.differences
                .iter()
                .map(|x| (&x.key, &x.details))
                .collect::<Vec<_>>()
        ));
    }
    let folder_db = "torture_folder";
    recreate(&e, d, folder_db).await;
    let target = schema(&e, folder_db).await;
    let diff = compare(&back, &target, d, &opts);
    let keys: Vec<String> = diff.differences.iter().map(|x| x.key.clone()).collect();
    let script = sync_script(&back, &target, &diff, &keys, d, &opts).script;
    let mut c = connect(&e, Some(folder_db)).await;
    let errors = run(&mut c, &script).await;
    c.close().await;
    let rebuilt = compare(&model, &schema(&e, folder_db).await, d, &opts);
    if !errors.is_empty() || !rebuilt.differences.is_empty() {
        problems.push(format!(
            "get latest from the folder: errors {errors:#?}, differences {:#?}",
            rebuilt
                .differences
                .iter()
                .map(|x| (&x.key, &x.details))
                .collect::<Vec<_>>()
        ));
    }
    sync(&e, d, none, empty, &mut problems).await;
    let left = schema(&e, empty).await;
    if !left.tables.is_empty() || !left.modules.is_empty() {
        problems.push(format!(
            "dropping everything left {:?} {:?}",
            left.tables.iter().map(|t| &t.name).collect::<Vec<_>>(),
            left.modules.iter().map(|m| &m.name).collect::<Vec<_>>()
        ));
    }
    for db in [a, b, empty, none, "torture_folder"] {
        let mut admin = connect(&e, None).await;
        let drop = match d {
            Dialect::MsSql => format!(
                "IF DB_ID('{db}') IS NOT NULL BEGIN ALTER DATABASE [{db}] SET SINGLE_USER WITH ROLLBACK IMMEDIATE; DROP DATABASE [{db}]; END"
            ),
            Dialect::Postgres => format!("DROP DATABASE IF EXISTS {db} WITH (FORCE)"),
            Dialect::MySql => format!("DROP DATABASE IF EXISTS {db}"),
        };
        admin.execute(&drop, &Limits::default()).await;
        admin.close().await;
    }
    assert!(
        problems.is_empty(),
        "{var}: {} problem(s):\n\n{}",
        problems.len(),
        problems.join("\n\n")
    );
}

#[tokio::test]
async fn mssql_awkward_schemas_sync() {
    torture("SQLOTUS_TEST_MSSQL", &MSSQL).await;
}

#[tokio::test]
async fn postgres_awkward_schemas_sync() {
    torture("SQLOTUS_TEST_PG", &POSTGRES).await;
}

#[tokio::test]
async fn mysql_awkward_schemas_sync() {
    torture("SQLOTUS_TEST_MYSQL", &MYSQL).await;
}
