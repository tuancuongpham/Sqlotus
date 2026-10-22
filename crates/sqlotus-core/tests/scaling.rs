//! Editor features must stay linear in the size of the script: a 2 MB data
//! script (20,000 INSERTs) once took 18 s to analyse after each edit, because
//! every statement scanned all the tokens of the script. Schema sync scripts
//! likewise in the size of the schema: 1,000 views on views took 10 s; and
//! completion in the size of the catalog: after JOIN, 8,000 tables took 17 s.
//!
//! Each feature runs on a script and on one four times longer; a linear
//! feature takes about four times as long, a quadratic one sixteen. The best
//! of three runs is compared, so a busy machine does not fail the test.

use std::hint::black_box;
use std::time::{Duration, Instant};

use sqlotus_core::catalog::{Catalog, Column, DbObject, ForeignKey, ObjectKind};
use sqlotus_core::format::FormatOptions;
use sqlotus_core::lint::LintOptions;
use sqlotus_core::safety::ScriptRun;
use sqlotus_core::schemadiff::{
    ColumnDef, CompareOptions, ForeignKeyDef, IndexDef, KeyDef, ModuleDef, SchemaModel, TableDef, compare, sync_script,
};
use sqlotus_core::{CompletionOptions, Dialect, analysis, lint, refactor, rename, scope, statements, variables};

/// `n` statements of each kind the features look at: plain INSERTs, and
/// statements with findings (SELECT *, = NULL, an unused variable).
fn script(n: usize) -> String {
    let mut s = String::new();
    for i in 0..n {
        s.push_str(&format!(
            "INSERT INTO dbo.Orders (Id, CustomerId, Total, Note) VALUES ({i}, {}, {}.50, N'note {i}');\n\
             DECLARE @v{i} int = {i};\nSELECT * FROM Orders o WHERE o.Note = NULL AND o.Id = {i};\n",
            i % 97,
            i % 1000
        ));
        if i % 100 == 99 {
            s.push_str("GO\n");
        }
    }
    s
}

fn catalog() -> Catalog {
    let col = |n: &str, t: &str| Column {
        name: n.into(),
        data_type: t.into(),
        nullable: true,
        is_primary_key: n == "Id",
        is_identity: false,
        is_computed: false,
    };
    Catalog {
        database: "Shop".into(),
        default_schema: "dbo".into(),
        schemas: vec!["dbo".into()],
        objects: vec![DbObject {
            schema: "dbo".into(),
            name: "Orders".into(),
            kind: ObjectKind::Table,
            columns: vec![
                col("Id", "int"),
                col("CustomerId", "int"),
                col("Total", "money"),
                col("Note", "nvarchar(50)"),
            ],
            params: vec![],
        }],
        foreign_keys: vec![],
    }
}

type Feature<'a> = Box<dyn Fn(&str) + 'a>;

fn feature<'a, T>(f: impl Fn(&str) -> T + 'a) -> Feature<'a> {
    Box::new(move |s| {
        black_box(f(s));
    })
}

fn best_of_three(f: &dyn Fn()) -> Duration {
    (0..3)
        .map(|_| {
            let t = Instant::now();
            f();
            t.elapsed()
        })
        .min()
        .unwrap()
}

#[test]
fn editor_features_scale_linearly() {
    const N: usize = 300;
    let d = Dialect::MsSql;
    let cat = catalog();
    let small = script(N);
    let large = script(4 * N);
    let features: Vec<(&str, Feature)> = vec![
        ("diagnostics", feature(|s| analysis::diagnostics(s, d, Some(&cat)))),
        (
            "lint",
            feature(|s| lint::lint(s, d, Some(&cat), &LintOptions::default())),
        ),
        (
            "ignore on this line",
            feature(|s| lint::ignore_edits(s, d, &analysis::diagnostics(s, d, None))),
        ),
        (
            "checks before running",
            feature(|s| {
                let run = ScriptRun::new(s, d);
                (run.changes(), run.warnings(), run.calls(), run.row_changes_only())
            }),
        ),
        (
            "undeclared variables",
            feature(|s| variables::undeclared_variables(s, d)),
        ),
        (
            "format",
            feature(|s| sqlotus_core::format_sql(s, d, &FormatOptions::default())),
        ),
        (
            "completion",
            feature(|s| sqlotus_core::complete(s, s.len() / 2, d, Some(&cat), &[], &CompletionOptions::default())),
        ),
        ("hover", feature(|s| analysis::hover(s, s.len() / 2, d, Some(&cat)))),
        ("rename", feature(|s| rename::rename_target(s, s.len() / 2, d))),
        (
            "execute current statement",
            feature(|s| statements::executable_statement_at(s, d, s.len() / 2)),
        ),
        ("symbols", feature(|s| scope::document_symbols(s, d))),
        (
            "expand wildcards",
            feature(|s| refactor::expand_wildcards(s, d, Some(&cat))),
        ),
        ("qualify names", feature(|s| refactor::qualify_object_names(s, d, &cat))),
        ("insert semicolons", feature(|s| refactor::insert_semicolons(s, d))),
    ];
    let mut slow = Vec::new();
    for (name, f) in &features {
        let a = best_of_three(&|| f(&small));
        let b = best_of_three(&|| f(&large));
        // too fast to measure reliably
        if b < Duration::from_millis(20) {
            continue;
        }
        let ratio = b.as_secs_f64() / a.as_secs_f64().max(1e-6);
        if ratio > 10.0 {
            slow.push(format!("{name}: {a:?} -> {b:?} (x{ratio:.1})"));
        }
    }
    assert!(slow.is_empty(), "grows faster than the script:\n{}", slow.join("\n"));
}

/// `n` tables, each with a key, an index and a foreign key to the one
/// before, and `n` views each built on the one before.
fn schema(n: usize, int_type: &str) -> SchemaModel {
    let mut m = SchemaModel::default();
    for i in 0..n {
        let col = |name: &str, ty: &str| ColumnDef {
            name: name.into(),
            data_type: ty.into(),
            ..Default::default()
        };
        m.tables.push(TableDef {
            schema: "dbo".into(),
            name: format!("T{i}"),
            columns: vec![col("Id", "int"), col("Ref", int_type), col("Name", "nvarchar(50)")],
            primary_key: Some(KeyDef {
                name: format!("PK_T{i}"),
                columns: vec!["Id".into()],
            }),
            indexes: vec![IndexDef {
                name: format!("IX_T{i}"),
                columns: vec!["Ref".into()],
                ..Default::default()
            }],
            foreign_keys: (i > 0)
                .then(|| ForeignKeyDef {
                    name: format!("FK_T{i}"),
                    columns: vec!["Id".into()],
                    ref_schema: "dbo".into(),
                    ref_table: format!("T{}", i - 1),
                    ref_columns: vec!["Id".into()],
                    ..Default::default()
                })
                .into_iter()
                .collect(),
            ..Default::default()
        });
        let below = if i == 0 {
            "T0".to_string()
        } else {
            format!("v{}", i - 1)
        };
        m.modules.push(ModuleDef {
            schema: "dbo".into(),
            name: format!("v{i}"),
            kind: ObjectKind::View,
            definition: format!("CREATE VIEW dbo.v{i} AS SELECT Id, Name FROM dbo.{below}"),
            table: None,
        });
    }
    m
}

#[test]
fn schema_sync_scales_linearly() {
    const N: usize = 150;
    let opts = CompareOptions::default();
    let mut slow = Vec::new();
    for d in [Dialect::MsSql, Dialect::Postgres] {
        // every table changes a column type; everything is created; everything is dropped
        let cases = |n: usize| {
            let (a, b) = (schema(n, "bigint"), schema(n, "int"));
            [
                (a.clone(), b),
                (a.clone(), SchemaModel::default()),
                (SchemaModel::default(), a),
            ]
        };
        for (k, (small, large)) in cases(N).into_iter().zip(cases(4 * N)).enumerate() {
            let run = |(src, tgt): &(SchemaModel, SchemaModel)| {
                let diff = compare(src, tgt, d, &opts);
                let keys: Vec<String> = diff.differences.iter().map(|x| x.key.clone()).collect();
                black_box(sync_script(src, tgt, &diff, &keys, d, &opts));
            };
            let a = best_of_three(&|| run(&small));
            let b = best_of_three(&|| run(&large));
            let ratio = b.as_secs_f64() / a.as_secs_f64().max(1e-6);
            if b >= Duration::from_millis(20) && ratio > 10.0 {
                slow.push(format!("{d:?} case {k}: {a:?} -> {b:?} (x{ratio:.1})"));
            }
        }
    }
    assert!(slow.is_empty(), "grows faster than the schema:\n{}", slow.join("\n"));
}

/// `n` tables of 20 columns, each with two foreign keys to earlier tables.
fn big_catalog(n: usize) -> Catalog {
    let col = |name: String| Column {
        is_primary_key: name == "Id",
        name,
        data_type: "int".into(),
        nullable: true,
        is_identity: false,
        is_computed: false,
    };
    let mut cat = Catalog {
        database: "Big".into(),
        default_schema: "dbo".into(),
        schemas: vec!["dbo".into()],
        objects: Vec::new(),
        foreign_keys: Vec::new(),
    };
    for i in 0..n {
        cat.objects.push(DbObject {
            schema: "dbo".into(),
            name: format!("Table{i}"),
            kind: ObjectKind::Table,
            columns: std::iter::once("Id".to_string())
                .chain((1..20).map(|c| format!("Col{c}")))
                .map(col)
                .collect(),
            params: vec![],
        });
        for k in 0..2.min(i) {
            cat.foreign_keys.push(ForeignKey {
                name: format!("FK_{i}_{k}"),
                from_schema: "dbo".into(),
                from_table: format!("Table{i}"),
                from_columns: vec!["Col1".into()],
                to_schema: "dbo".into(),
                to_table: format!("Table{}", (i * 7 + k) % i),
                to_columns: vec!["Id".into()],
            });
        }
    }
    cat
}

#[test]
fn completion_scales_with_the_catalog() {
    let d = Dialect::MsSql;
    let mut slow = Vec::new();
    let (small, large) = (big_catalog(500), big_catalog(2000));
    for src in [
        "SELECT * FROM Table1 a JOIN Table2 b ON b.Col1 = a.Id JOIN ",
        "SELECT * FROM Ta",
        "SELECT  FROM Table1 a JOIN Table2 b ON 1 = 1",
    ] {
        let at = if src.starts_with("SELECT  FROM") { 7 } else { src.len() };
        let run = |cat: &Catalog| {
            black_box(sqlotus_core::complete(
                src,
                at,
                d,
                Some(cat),
                &[],
                &CompletionOptions::default(),
            ));
        };
        let a = best_of_three(&|| run(&small));
        let b = best_of_three(&|| run(&large));
        let ratio = b.as_secs_f64() / a.as_secs_f64().max(1e-6);
        if b >= Duration::from_millis(20) && ratio > 10.0 {
            slow.push(format!("{src:?}: {a:?} -> {b:?} (x{ratio:.1})"));
        }
    }
    assert!(slow.is_empty(), "grows faster than the catalog:\n{}", slow.join("\n"));
}

/// One-line scripts (minified, or pasted from an application's log) are
/// lexed on every keystroke: a line of `:name` parameters once took 10 s at
/// half a megabyte, as each `:` looked back to the start of its line for a
/// SQLCMD command. SQLCMD scripts and the scripts made from table data must
/// grow no faster than their input either.
#[test]
fn long_lines_and_new_scripts_scale_linearly() {
    use sqlotus_core::datascript::data_script;
    use sqlotus_core::sqlcmd::Sqlcmd;
    let d = Dialect::MsSql;
    let params = |n: usize| format!("INSERT INTO t VALUES {}", "(:id, :name), ".repeat(n));
    let open_vars = |n: usize| format!("SELECT {}", "a$(b ".repeat(n));
    let sqlcmd = |n: usize| {
        let mut s = String::new();
        for i in 0..n {
            s.push_str(&format!(
                ":setvar V{i} \"value {i}\"\nSELECT $(V{i}), N'$(V{i})' FROM dbo.T_$(V{i});\nGO\n"
            ));
        }
        s
    };
    let table = &catalog().objects[0];
    let rows = |n: usize| -> Vec<Vec<Option<String>>> {
        (0..n)
            .map(|i| {
                vec![
                    Some(i.to_string()),
                    Some((i % 7).to_string()),
                    Some(format!("{i}.50")),
                    Some(format!("note '{i}'")),
                ]
            })
            .collect()
    };
    let cases: Vec<(&str, Feature)> = vec![
        (
            "tokenize :name parameters",
            feature(|s| sqlotus_core::tokenize(s, d).len()),
        ),
        (
            "analysis of :name parameters",
            feature(|s| analysis::diagnostics(s, d, None)),
        ),
    ];
    let mut slow = Vec::new();
    for (name, f) in &cases {
        let (a, b) = (params(5_000), params(20_000));
        let (ta, tb) = (best_of_three(&|| f(&a)), best_of_three(&|| f(&b)));
        if tb >= Duration::from_millis(20) && tb.as_secs_f64() / ta.as_secs_f64().max(1e-6) > 10.0 {
            slow.push(format!("{name}: {ta:?} -> {tb:?}"));
        }
    }
    let ratio = |small: &dyn Fn(), large: &dyn Fn()| {
        let (ta, tb) = (best_of_three(small), best_of_three(large));
        (tb >= Duration::from_millis(20) && tb.as_secs_f64() / ta.as_secs_f64().max(1e-6) > 10.0)
            .then(|| format!("{ta:?} -> {tb:?}"))
    };
    let (a, b) = (open_vars(20_000), open_vars(80_000));
    if let Some(r) = ratio(&|| drop(black_box(sqlotus_core::tokenize(&a, d))), &|| {
        drop(black_box(sqlotus_core::tokenize(&b, d)))
    }) {
        slow.push(format!("tokenize unclosed $(: {r}"));
    }
    let (a, b) = (sqlcmd(500), sqlcmd(2_000));
    if let Some(r) = ratio(&|| drop(black_box(Sqlcmd::parse(&a))), &|| {
        drop(black_box(Sqlcmd::parse(&b)))
    }) {
        slow.push(format!("SQLCMD variables: {r}"));
    }
    let (a, b) = (rows(5_000), rows(20_000));
    if let Some(r) = ratio(&|| drop(black_box(data_script(table, d, &a, usize::MAX - 1))), &|| {
        drop(black_box(data_script(table, d, &b, usize::MAX - 1)))
    }) {
        slow.push(format!("data as INSERT: {r}"));
    }
    assert!(slow.is_empty(), "grows faster than the input:\n{}", slow.join("\n"));
}
