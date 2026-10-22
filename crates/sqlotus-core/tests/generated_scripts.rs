//! The scripts Sqlotus writes from names and values it does not control —
//! table data as INSERT, the value search, backup and restore, the query
//! that follows a foreign key, SQLCMD variables — keep every name and value
//! inside its quotes: whatever they hold, a script has exactly the
//! statements it should, and one meant to read only reads.

mod support;

use support::{DIALECTS, Rng, catalog};

use sqlotus_core::Dialect;
use sqlotus_core::backup::{BackupKind, BackupOptions, RestoreMove, RestoreOptions, backup_script, restore_script};
use sqlotus_core::catalog::{Column, DbObject, ObjectKind};
use sqlotus_core::datascript::{DataScriptOptions, data_query, data_script};
use sqlotus_core::findvalue::{FindValueOptions, MatchMode, find_value_script};
use sqlotus_core::lexer::TokenKind;
use sqlotus_core::links::{link_query, result_links};
use sqlotus_core::safety::ScriptRun;
use sqlotus_core::sqlcmd::Sqlcmd;
use sqlotus_core::statements::{split_batches, split_statements};
use sqlotus_core::tokenize;

/// Pieces of awkward text: every kind of quote, escapes, comment starts,
/// dollar quotes, statement ends, GO lines, wildcards, SQLCMD syntax and
/// Unicode.
const PIECES: &[&str] = &[
    "'",
    "''",
    "\"",
    "`",
    "]",
    "[",
    "\\",
    "\\'",
    "--",
    "/*",
    "*/",
    "$$",
    "$x$",
    ";",
    "\nGO\n",
    "\r\nGO 2\r\n",
    "%",
    "_",
    "!",
    "?",
    "\n:setvar x 1\n",
    "$(x)",
    "N'",
    "E'",
    "😀",
    "Nguyễn",
    " DROP TABLE t; ",
    "\t",
    "\n",
    "0x41",
    "b'1'",
    "1e5",
    "-",
    "abc",
];

fn nasty(rng: &mut Rng) -> String {
    (0..1 + rng.below(6)).map(|_| *rng.pick(PIECES)).collect()
}

/// `;` tokens and statement-level changes of a script.
fn semicolons(script: &str, d: Dialect) -> usize {
    tokenize(script, d)
        .iter()
        .filter(|t| t.kind == TokenKind::Semicolon)
        .count()
}

#[test]
fn the_value_search_only_reads_whatever_the_value() {
    let cat = catalog();
    let mut rng = Rng::new(7);
    for d in DIALECTS {
        for _ in 0..400 {
            let opts = FindValueOptions {
                value: nasty(&mut rng),
                mode: *rng.pick(&[MatchMode::Contains, MatchMode::StartsWith, MatchMode::Exact]),
                schema: String::new(),
                ignore_case: rng.chance(50),
            };
            let script = find_value_script(&cat, d, &opts).unwrap();
            let run = ScriptRun::new(&script, d);
            assert!(
                run.changes().is_empty() && run.calls().is_empty(),
                "{d:?} {:?}:\n{script}",
                opts.value
            );
            // one SELECT, ended by the one `;`, in one batch
            assert_eq!(semicolons(&script, d), 1, "{d:?} {:?}:\n{script}", opts.value);
            assert_eq!(split_batches(&script, d).len(), 1, "{d:?} {:?}", opts.value);
            assert_eq!(Sqlcmd::parse(&script), Ok(None), "{d:?} {:?}", opts.value);
        }
    }
}

fn typed_table() -> DbObject {
    let c = |name: &str, ty: &str, identity: bool| Column {
        name: name.into(),
        data_type: ty.into(),
        nullable: true,
        is_primary_key: name == "Id",
        is_identity: identity,
        is_computed: false,
    };
    DbObject {
        schema: "dbo".into(),
        name: "Typed".into(),
        kind: ObjectKind::Table,
        columns: vec![
            c("Id", "int", true),
            c("Note", "nvarchar(max)", false),
            c("Code", "varchar(20)", false),
            c("Amount", "decimal(10,2)", false),
            c("Ratio", "float", false),
            c("Flag", "bit", false),
            c("Bits", "bit(8)", false),
            c("Blob", "varbinary(max)", false),
            c("At", "datetime", false),
            c("Doc", "xml", false),
        ],
        params: vec![],
    }
}

#[test]
fn table_data_keeps_every_value_in_its_literal() {
    let table = typed_table();
    let mut rng = Rng::new(11);
    for d in DIALECTS {
        for _ in 0..200 {
            let rows: Vec<Vec<Option<String>>> = (0..1 + rng.below(3))
                .map(|_| {
                    table
                        .columns
                        .iter()
                        .map(|_| (!rng.chance(10)).then(|| nasty(&mut rng)))
                        .collect()
                })
                .collect();
            let script = data_script(&table, d, &rows, 100);
            // per INSERT one `;`, plus SET IDENTITY_INSERT ON / OFF (SQL Server) or
            // the sequence reset (PostgreSQL); nothing a value brings
            let expected = 1 + match d {
                Dialect::MsSql => 2,
                Dialect::Postgres => 1,
                Dialect::MySql => 0,
            };
            assert_eq!(semicolons(&script.script, d), expected, "{d:?}:\n{}", script.script);
            assert_eq!(split_batches(&script.script, d).len(), 1, "{d:?}:\n{}", script.script);
            let changes = ScriptRun::new(&script.script, d).changes();
            assert!(
                changes.iter().all(|c| c.starts_with("INSERT") || c == "setval"),
                "{d:?} {changes:?}:\n{}",
                script.script
            );
        }
    }
}

#[test]
fn a_condition_cannot_add_statements_to_the_data_query() {
    let table = typed_table();
    let mut rng = Rng::new(13);
    for d in DIALECTS {
        for _ in 0..300 {
            let filter = format!("Id > 0{}", nasty(&mut rng));
            let opts = DataScriptOptions { max_rows: 10, filter };
            if let Ok(sql) = data_query(&table, d, &opts) {
                let run = ScriptRun::new(&sql, d);
                assert!(run.changes().is_empty() && run.calls().is_empty(), "{d:?}:\n{sql}");
                assert_eq!(split_statements(&sql, d).len(), 1, "{d:?}:\n{sql}");
            }
        }
    }
}

#[test]
fn backup_and_restore_scripts_keep_names_and_paths_quoted() {
    let mut rng = Rng::new(17);
    for _ in 0..300 {
        let database = format!("Shop{}", nasty(&mut rng));
        let path = format!("D:\\Backups\\{}.bak", nasty(&mut rng));
        let kind = *rng.pick(&[BackupKind::Full, BackupKind::Differential, BackupKind::Log]);
        let backup = backup_script(&BackupOptions {
            database: database.clone(),
            kind,
            path: path.clone(),
            copy_only: rng.chance(50),
            compression: rng.chance(50),
            checksum: rng.chance(50),
            overwrite: rng.chance(50),
            verify: rng.chance(50),
        })
        .unwrap();
        let changes = ScriptRun::new(&backup, Dialect::MsSql).changes();
        assert!(
            changes.len() == 1 && changes[0].starts_with("BACKUP"),
            "{changes:?}:\n{backup}"
        );
        assert!(split_batches(&backup, Dialect::MsSql).len() <= 2, "{backup}");
        let restore = restore_script(&RestoreOptions {
            path,
            file_number: 1,
            database,
            moves: vec![RestoreMove {
                logical_name: nasty(&mut rng),
                path: format!(r"E:\Data\{}", nasty(&mut rng)),
            }],
            replace: rng.chance(50),
            recovery: rng.chance(50),
            close_connections: rng.chance(50),
        })
        .unwrap();
        let changes = ScriptRun::new(&restore, Dialect::MsSql).changes();
        assert!(
            changes
                .iter()
                .all(|c| c.starts_with("RESTORE") || c.starts_with("ALTER DATABASE")),
            "{changes:?}:\n{restore}"
        );
        assert_eq!(
            changes.iter().filter(|c| c.starts_with("RESTORE")).count(),
            1,
            "{restore}"
        );
        assert!(split_batches(&restore, Dialect::MsSql).len() <= 4, "{restore}");
    }
}

#[test]
fn following_a_foreign_key_reads_one_table() {
    let cat = catalog();
    let cols = ["Id".to_string(), "CustomerId".to_string(), "NgàyĐặt".to_string()];
    let mut rng = Rng::new(19);
    for d in DIALECTS {
        let links = result_links("SELECT * FROM dbo.Orders", d, &cat, &cols);
        assert!(!links.is_empty(), "{d:?}");
        for _ in 0..300 {
            for link in &links {
                let values: Vec<Option<String>> = link.columns.iter().map(|_| Some(nasty(&mut rng))).collect();
                if let Ok(sql) = link_query(link, &values, &cat, d) {
                    let run = ScriptRun::new(&sql, d);
                    assert!(run.changes().is_empty() && run.calls().is_empty(), "{d:?}:\n{sql}");
                    assert_eq!(split_statements(&sql, d).len(), 1, "{d:?}:\n{sql}");
                    assert_eq!(semicolons(&sql, d), 0, "{d:?}:\n{sql}");
                }
            }
        }
    }
}

#[test]
fn sqlcmd_scripts_of_any_shape_expand_without_panics() {
    let mut rng = Rng::new(23);
    let lines = [
        ":setvar A \"x\"",
        ":setvar B $(A)y",
        ":setvar C",
        ":on error exit",
        ":r other.sql",
        "SELECT '$(A)', $(B) FROM t_$(A);",
        "GO",
        "GO 2",
        "-- $(nothing)",
        "/* :setvar Z 1 */",
        "SELECT N'Nguyễn $(A)';",
        "  :setvar D \"unclosed",
    ];
    for _ in 0..2000 {
        let script: Vec<&str> = (0..1 + rng.below(8)).map(|_| *rng.pick(&lines)).collect();
        let script = script.join(if rng.chance(30) { "\r\n" } else { "\n" });
        if let Ok(Some(cmd)) = Sqlcmd::parse(&script) {
            assert_eq!(cmd.text.len(), script.len());
            for b in split_batches(&cmd.text, Dialect::MsSql) {
                let text = cmd.expand(b.span.start..b.span.end);
                // no command line reaches the server ("/* :setvar */" is a comment)
                assert!(
                    !text.contains("$(A)") && !text.lines().any(|l| l.trim_start().starts_with(':')),
                    "{script:?} -> {text:?}"
                );
            }
        }
    }
}

/// An object named to break out of a comment: a schema sync and generated
/// test data name objects in their comments and warnings.
const BREAKOUT: &str = "x\nDROP DATABASE prod;\n--";

#[test]
fn object_names_cannot_turn_comments_into_statements() {
    use sqlotus_core::schemadiff::{ColumnDef, CompareOptions, SchemaModel, TableDef, compare, sync_script};
    use sqlotus_core::testdata::{DataContext, TestDataOptions, insert_script, plan_columns};
    let opts = CompareOptions::default();
    let injected = |script: &str, d: Dialect| {
        ScriptRun::new(script, d)
            .changes()
            .iter()
            .any(|c| c.starts_with("DROP DATABASE"))
    };
    for d in DIALECTS {
        // a column whose type narrows and whose identity changes: warnings
        // and a "recreate by hand" comment name it
        let table = |ty: &str, identity: bool| TableDef {
            schema: format!("s{BREAKOUT}"),
            name: format!("T{BREAKOUT}"),
            columns: vec![ColumnDef {
                name: format!("c{BREAKOUT}"),
                data_type: ty.into(),
                identity,
                ..Default::default()
            }],
            ..Default::default()
        };
        let src = SchemaModel {
            tables: vec![table("int", true)],
            ..Default::default()
        };
        let tgt = SchemaModel {
            tables: vec![table("bigint", false)],
            ..Default::default()
        };
        let diff = compare(&src, &tgt, d, &opts);
        let keys: Vec<String> = diff.differences.iter().map(|x| x.key.clone()).collect();
        let sync = sync_script(&src, &tgt, &diff, &keys, d, &opts);
        assert!(!injected(&sync.script, d), "{d:?}:\n{}", sync.script);
        // dropping the table only in the target warns with its name too
        let drop = sync_script(
            &SchemaModel::default(),
            &tgt,
            &compare(&SchemaModel::default(), &tgt, d, &opts),
            &keys,
            d,
            &opts,
        );
        assert!(!injected(&drop.script, d), "{d:?}:\n{}", drop.script);

        let t = DbObject {
            schema: format!("s{BREAKOUT}"),
            name: format!("T{BREAKOUT}"),
            kind: ObjectKind::Table,
            columns: vec![Column {
                name: format!("c{BREAKOUT}"),
                data_type: "int".into(),
                nullable: false,
                is_primary_key: false,
                is_identity: false,
                is_computed: false,
            }],
            params: vec![],
        };
        let plans = plan_columns(&t, &[], d);
        let script = insert_script(&t, &plans, &[], &DataContext::default(), &TestDataOptions::default(), d).unwrap();
        assert!(!injected(&script, d), "{d:?}:\n{script}");
        let data = data_script(&t, d, &[vec![Some("1".into())]], 10);
        assert!(!injected(&data.script, d), "{d:?}:\n{}", data.script);
    }
}
