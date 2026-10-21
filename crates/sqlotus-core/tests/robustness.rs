//! Every editor-path function must cope with half-typed, malformed and
//! multi-byte text: run each one on every prefix of tricky inputs, at every
//! cursor position, in every dialect. Any panic fails the test.

use sqlotus_core::catalog::{Catalog, Column, DbObject, ForeignKey, ObjectKind, Param};
use sqlotus_core::format::KeywordCase;
use sqlotus_core::{
    CompletionOptions, Dialect, FormatOptions, analysis, complete, format_sql, plan, refactor, rename, statements,
    utf16_to_byte, variables,
};

fn col(name: &str, ty: &str, nullable: bool) -> Column {
    Column {
        name: name.into(),
        data_type: ty.into(),
        nullable,
        is_primary_key: name == "Id",
        is_identity: name == "Id",
        is_computed: false,
    }
}

fn catalog() -> Catalog {
    Catalog {
        database: "Shop".into(),
        default_schema: "dbo".into(),
        schemas: vec!["dbo".into()],
        objects: vec![
            DbObject {
                schema: "dbo".into(),
                name: "Customers".into(),
                kind: ObjectKind::Table,
                columns: vec![
                    col("Id", "int", false),
                    col("Code", "varchar(10)", true),
                    col("Tên", "nvarchar(50)", true),
                ],
                params: vec![],
            },
            DbObject {
                schema: "dbo".into(),
                name: "Orders".into(),
                kind: ObjectKind::Table,
                columns: vec![
                    col("Id", "int", false),
                    col("CustomerId", "int", false),
                    col("NgàyĐặt", "datetime2", false),
                ],
                params: vec![],
            },
            DbObject {
                schema: "dbo".into(),
                name: "GetOrders".into(),
                kind: ObjectKind::Procedure,
                columns: vec![],
                params: vec![Param {
                    name: "@CustomerId".into(),
                    data_type: "int".into(),
                    is_output: false,
                    has_default: false,
                }],
            },
        ],
        foreign_keys: vec![ForeignKey {
            name: "FK_Orders_Customers".into(),
            from_schema: "dbo".into(),
            from_table: "Orders".into(),
            from_columns: vec!["CustomerId".into()],
            to_schema: "dbo".into(),
            to_table: "Customers".into(),
            to_columns: vec!["Id".into()],
        }],
    }
}

const INPUTS: &[&str] = &[
    "SELECT c.Id, c.[Tên] FROM dbo.Customers c JOIN dbo.Orders o ON o.CustomerId = c.Id WHERE YEAR(o.NgàyĐặt) = 2024 AND c.Code = N'Á'",
    "UPDATE dbo.Customers SET Code = 'x' WHERE Id NOT IN (SELECT CustomerId FROM dbo.Orders); DELETE FROM dbo.Orders",
    "DECLARE @a int = 1, @b nvarchar(10) = N'ễ'\nGO\nSELECT @a, @b, @@ROWCOUNT\nGO 3\nEXEC dbo.GetOrders @CustomerId = @a",
    "BEGIN TRAN; INSERT INTO dbo.Customers (Code) VALUES ('a'), (N'ươ'); COMMIT",
    "WITH x AS (SELECT * FROM dbo.Orders) SELECT * FROM x WHERE CAST(x.Id AS varchar(5)) = 1 --ghi chú\n/* chưa đóng",
    "SELECT 'chuỗi chưa đóng FROM dbo.Customers WHERE [Tên = 1",
    "CREATE PROCEDURE dbo.p @Id int AS BEGIN SELECT @Id; SELECT @Khac END",
    "select $1, \"Tên\" from public.t where x::int = 1 and y = E'\\n' and z = $$ thân $$",
    "SET @x := 1; SELECT `cột` FROM t WHERE id = @y; SELECT 1 INTO @z",
    "MERGE dbo.Customers AS t USING (SELECT 1 AS Id) s ON t.Id = s.Id WHEN MATCHED THEN UPDATE SET Code = 'a' WHEN NOT MATCHED THEN INSERT (Code) VALUES ('b');",
    "SELECT ( ( ( ] ) ; ; , . .. @ @@ # N' 😀 ẫ",
];

#[test]
fn editor_functions_never_panic_on_partial_text() {
    let cat = catalog();
    let snippets = sqlotus_core::snippet::builtin_snippets();
    let opts = CompletionOptions::default();
    let fmt = FormatOptions::default();
    for full in INPUTS {
        // every prefix that ends on a char boundary
        let cuts: Vec<usize> = full.char_indices().map(|(i, _)| i).chain([full.len()]).collect();
        for &cut in &cuts {
            let text = &full[..cut];
            for d in [Dialect::MsSql, Dialect::Postgres, Dialect::MySql] {
                let _ = analysis::diagnostics(text, d, Some(&cat));
                let _ = analysis::diagnostics(text, d, None);
                let _ = analysis::execution_warnings(text, d);
                let _ = analysis::modifying_statements(text, d);
                let _ = analysis::row_changes_only(text, d);
                let _ = variables::undeclared_variables(text, d);
                let _ = variables::prelude(
                    text,
                    d,
                    &[variables::VariableValue {
                        name: "@a".into(),
                        data_type: "int".into(),
                        value: Some("1".into()),
                    }],
                );
                let _ = format_sql(text, d, &fmt);
                let _ = refactor::expand_wildcards(text, d, Some(&cat));
                let _ = refactor::qualify_object_names(text, d, &cat);
                let _ = refactor::insert_semicolons(text, d);
                let _ = refactor::remove_brackets(text, d);
                let _ = refactor::apply_keyword_case(text, d, KeywordCase::Upper);
                let _ = statements::split_batches(text, d);
                let _ = statements::split_statements(text, d);
            }
            // cursor positions in the full text are covered below; here only
            // the end of each prefix (where the user is typing)
            let d = Dialect::MsSql;
            let _ = complete(text, text.len(), d, Some(&cat), &snippets, &opts);
            let _ = analysis::hover(text, text.len(), d, Some(&cat));
            let _ = analysis::signature_help(text, text.len(), d, Some(&cat));
        }
        // every cursor position (including inside multi-byte chars) in the full text
        for off16 in 0..=full.encode_utf16().count() + 1 {
            let off = utf16_to_byte(full, off16);
            for d in [Dialect::MsSql, Dialect::Postgres, Dialect::MySql] {
                let r = complete(full, off, d, Some(&cat), &snippets, &opts);
                assert!(r.replace_start <= r.replace_end && r.replace_end <= full.len());
                let _ = analysis::hover(full, off, d, Some(&cat));
                let _ = analysis::signature_help(full, off, d, Some(&cat));
                let _ = analysis::object_at(full, off, d, &cat);
                let _ = statements::statement_at(full, d, off);
                if let Some(t) = rename::rename_target(full, off, d) {
                    let _ = rename::rename(full, off, d, &format!("{}_mới", t.name));
                }
            }
        }
        for off in 0..=full.len() + 2 {
            let _ = complete(full, off, Dialect::MsSql, Some(&cat), &snippets, &opts);
            let _ = analysis::hover(full, off, Dialect::MsSql, Some(&cat));
            let _ = rename::rename_target(full, off, Dialect::MsSql);
        }
    }
}

#[test]
fn plan_parsers_reject_garbage_without_panicking() {
    for junk in [
        "",
        "<",
        "<ShowPlanXML/>",
        "<a><StmtSimple><QueryPlan><RelOp/></QueryPlan></StmtSimple></a>",
        "not xml ẫ",
    ] {
        let _ = plan::parse_showplan_xml(junk);
    }
    for junk in [
        "",
        "[]",
        "[{}]",
        "[{\"Plan\": 1}]",
        "{",
        "[{\"Plan\":{\"Plans\":[{\"Plans\":null}]}}]",
    ] {
        let _ = plan::parse_pg_json("x", junk);
    }
    for junk in [
        "",
        "->",
        "-> ",
        "    -> x  (cost=",
        "-> a\n-> b\n        -> c\n  -> d",
        "ẫ -> ẫ  (rows=ẫ)",
    ] {
        let _ = plan::parse_mysql_tree("x", junk);
    }
    let _ = plan::parse_mysql_table("x", &[], &[vec![None]]);
}
