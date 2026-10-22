//! Editing query results: which results can be edited (one table with a
//! primary key, plain columns), and the checked INSERT / UPDATE / DELETE
//! statements for the edits. UPDATE and DELETE also compare the values the
//! user saw, so a row changed by someone else in the meantime is not
//! overwritten.

use std::collections::HashMap;

use serde::{Deserialize, Serialize};

use crate::Dialect;
use crate::catalog::{Catalog, ObjectKind};
use crate::lexer::{Token, TokenKind, significant, tokenize, word};
use crate::scope::analyze_statement;
use crate::sqltype::SqlType;
use crate::statements::split_statements;

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct EditColumn {
    /// Column of the table this result column shows; None for expressions.
    pub column: Option<String>,
    pub data_type: String,
    pub nullable: bool,
    pub is_key: bool,
    /// Identity / computed / rowversion: shown, never written.
    pub read_only: bool,
}

/// Where an editable result comes from: one column entry per result column.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct EditSource {
    pub schema: String,
    pub table: String,
    pub columns: Vec<EditColumn>,
}

const NOT_EDITABLE: &[&str] = &[
    "JOIN",
    "UNION",
    "INTERSECT",
    "EXCEPT",
    "GROUP",
    "HAVING",
    "DISTINCT",
    "INTO",
    "PIVOT",
    "UNPIVOT",
    "APPLY",
    "OVER",
    "FOR",
];

/// The table behind `sql` if its result can be edited row by row.
/// `result_columns` are the names of the columns the server returned.
pub fn editable_source(
    sql: &str,
    dialect: Dialect,
    catalog: &Catalog,
    result_columns: &[String],
) -> Result<EditSource, String> {
    let spans: Vec<_> = split_statements(sql, dialect)
        .into_iter()
        .filter(|s| {
            tokenize(s.text(sql), dialect)
                .iter()
                .any(|t| !t.is_trivia() && t.kind != TokenKind::Semicolon)
        })
        .collect();
    if spans.len() != 1 {
        return Err("Only the result of a single SELECT statement can be edited.".into());
    }
    let tokens = tokenize(sql, dialect);
    let stmt: Vec<Token<'_>> = tokens
        .iter()
        .filter(|t| t.start >= spans[0].start && t.end() <= spans[0].end)
        .copied()
        .collect();
    let t = significant(&stmt);
    if word(t.first()) != "SELECT" {
        return Err("Only SELECT results can be edited.".into());
    }
    let mut depth = 0i32;
    for x in &t {
        match x.kind {
            TokenKind::LParen => depth += 1,
            TokenKind::RParen => depth -= 1,
            TokenKind::Word if depth == 0 && NOT_EDITABLE.contains(&x.upper().as_str()) => {
                return Err(format!(
                    "A result with {} cannot be edited: it must come from one table, row for row.",
                    x.upper()
                ));
            }
            _ => {}
        }
    }
    let scope = analyze_statement(&stmt, dialect);
    if scope.tables.len() != 1 || scope.tables[0].local || scope.tables[0].is_call {
        return Err("The result must come from exactly one table.".into());
    }
    let r = &scope.tables[0];
    let table = catalog
        .find(r.schema.as_deref(), &r.name)
        .filter(|o| o.kind == ObjectKind::Table)
        .ok_or_else(|| {
            format!(
                "{} is not a table in the loaded objects (views cannot be edited; reload objects if it is new).",
                r.name
            )
        })?;
    let keys: Vec<&str> = table
        .columns
        .iter()
        .filter(|c| c.is_primary_key)
        .map(|c| c.name.as_str())
        .collect();
    if keys.is_empty() {
        return Err(format!(
            "{}.{} has no primary key, so rows cannot be identified safely.",
            table.schema, table.name
        ));
    }

    let items = select_items(&t).ok_or("SELECT without FROM cannot be edited.")?;
    let qualifier_ok = |q: &Option<Token<'_>>| q.is_none_or(|q| r.matches_qualifier(&q.ident_value()));
    let mut sources: Vec<Option<String>> = Vec::new();
    for item in &items {
        match item {
            SelectItem::Star(q) if qualifier_ok(q) => {
                sources.extend(table.columns.iter().map(|c| Some(c.name.clone())))
            }
            SelectItem::Column(q, c) if qualifier_ok(q) => sources.push(
                table
                    .columns
                    .iter()
                    .find(|x| x.name.eq_ignore_ascii_case(&c.ident_value()))
                    .map(|c| c.name.clone()),
            ),
            _ => sources.push(None),
        }
    }
    if sources.len() != result_columns.len() {
        return Err(
            "The result columns do not match the table's columns; reload objects and run the query again.".into(),
        );
    }
    let columns: Vec<EditColumn> = sources
        .iter()
        .map(|s| {
            let c = s.as_ref().and_then(|n| table.columns.iter().find(|x| &x.name == n));
            let ty = c.map(|c| c.data_type.to_ascii_lowercase()).unwrap_or_default();
            EditColumn {
                column: s.clone(),
                data_type: c.map(|c| c.data_type.clone()).unwrap_or_default(),
                nullable: c.is_some_and(|c| c.nullable),
                is_key: c.is_some_and(|c| c.is_primary_key),
                read_only: c.is_none_or(|c| {
                    c.is_identity
                        || c.is_computed
                        || ty.starts_with("timestamp") && dialect == Dialect::MsSql
                        || ty.starts_with("rowversion")
                }),
            }
        })
        .collect();
    for k in keys {
        if !columns.iter().any(|c| c.column.as_deref() == Some(k)) {
            return Err(format!(
                "The primary key column {k} must be in the result to edit rows."
            ));
        }
    }
    Ok(EditSource {
        schema: table.schema.clone(),
        table: table.name.clone(),
        columns,
    })
}

/// What an item of a SELECT list shows.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum SelectItem<'a> {
    /// `*` or `q.*`.
    Star(Option<Token<'a>>),
    /// `[q.]col [[AS] alias]`.
    Column(Option<Token<'a>>, Token<'a>),
    /// Anything else.
    Expression,
}

/// The items of a SELECT's list, between SELECT [ALL | DISTINCT] [TOP n]
/// and FROM. `t`: the significant tokens of the statement, from SELECT on.
/// `None` without a FROM.
pub(crate) fn select_items<'a>(t: &[Token<'a>]) -> Option<Vec<SelectItem<'a>>> {
    let mut i = 1;
    match word(t.get(i)).as_str() {
        "ALL" => i += 1,
        "DISTINCT" => {
            i += 1;
            // PostgreSQL DISTINCT ON (...)
            if word(t.get(i)) == "ON" && t.get(i + 1).is_some_and(|x| x.kind == TokenKind::LParen) {
                i = crate::lexer::matching_paren(t, i + 1)? + 1;
            }
        }
        _ => {}
    }
    if word(t.get(i)) == "TOP" {
        i += if t.get(i + 1).is_some_and(|x| x.kind == TokenKind::LParen) {
            4
        } else {
            2
        };
        if word(t.get(i)) == "PERCENT" {
            i += 1;
        }
        if word(t.get(i)) == "WITH" && word(t.get(i + 1)) == "TIES" {
            i += 2;
        }
    }
    let mut depth = 0i32;
    let mut from = None;
    for (k, x) in t.iter().enumerate().skip(i) {
        match x.kind {
            TokenKind::LParen => depth += 1,
            TokenKind::RParen => depth -= 1,
            _ if depth == 0 && word(Some(x)) == "FROM" => {
                from = Some(k);
                break;
            }
            _ => {}
        }
    }
    let mut items: Vec<Vec<Token<'a>>> = vec![Vec::new()];
    let mut d = 0i32;
    for x in &t[i.min(from?)..from?] {
        match x.kind {
            TokenKind::LParen => d += 1,
            TokenKind::RParen => d -= 1,
            TokenKind::Comma if d == 0 => {
                items.push(Vec::new());
                continue;
            }
            _ => {}
        }
        items.last_mut().expect("non-empty").push(*x);
    }
    Some(
        items
            .iter()
            .map(|item| match item.as_slice() {
                [star] if star.text == "*" => SelectItem::Star(None),
                [q, dot, star] if dot.kind == TokenKind::Dot && star.text == "*" && q.is_name() => {
                    SelectItem::Star(Some(*q))
                }
                _ => {
                    let (q, col, rest) = match item.as_slice() {
                        [q, dot, c, rest @ ..] if dot.kind == TokenKind::Dot && q.is_name() && c.is_name() => {
                            (Some(*q), Some(*c), rest)
                        }
                        [c, rest @ ..] if c.is_name() => (None, Some(*c), rest),
                        _ => (None, None, &item[..0]),
                    };
                    let plain = match rest {
                        [] => true,
                        [alias] => alias.is_name(),
                        [as_, alias] => word(Some(as_)) == "AS" && alias.is_name(),
                        _ => false,
                    };
                    match col {
                        Some(c) if plain => SelectItem::Column(q, c),
                        _ => SelectItem::Expression,
                    }
                }
            })
            .collect(),
    )
}

/// One edit of a result row. Values are keyed by table column name.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "camelCase")]
pub enum RowEdit {
    /// `original`: the row as loaded; `values`: the changed columns.
    Update {
        original: HashMap<String, Option<String>>,
        values: HashMap<String, Option<String>>,
    },
    Insert {
        values: HashMap<String, Option<String>>,
    },
    Delete {
        original: HashMap<String, Option<String>>,
    },
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct EditPlan {
    /// One statement per edit, in order.
    pub statements: Vec<String>,
    /// Values that do not fit their column; nothing should run while any.
    pub problems: Vec<String>,
}

/// Types whose text form round-trips exactly, so WHERE col = <value seen> is reliable.
fn comparable(t: &str) -> bool {
    let ty = SqlType::parse(t);
    (ty.is_number() && !ty.is_floating())
        || matches!(
            ty.base(),
            "char"
                | "varchar"
                | "nchar"
                | "nvarchar"
                | "character"
                | "character varying"
                | "bit"
                | "bool"
                | "boolean"
                | "date"
                | "uniqueidentifier"
                | "uuid"
        )
}

/// `v` as a SQL literal for column `col`; values that do not fit it are
/// described in `problems`.
pub(crate) fn literal(v: &Option<String>, col: &EditColumn, dialect: Dialect, problems: &mut Vec<String>) -> String {
    let name = col.column.as_deref().unwrap_or("?");
    let Some(v) = v else {
        if !col.nullable {
            problems.push(format!("{name} cannot be NULL."));
        }
        return "NULL".into();
    };
    let ty = SqlType::parse(&col.data_type);
    let b = ty.base();
    // PostgreSQL shows money in the server's currency format ($1,234.56),
    // which it also reads back
    if dialect == Dialect::Postgres && b == "money" {
        return dialect.string_literal(v.trim());
    }
    if ty.is_number() {
        let s = v.trim();
        // a plain number only: `NaN` or `inf` would reach the server as SQL text
        let plain = s
            .chars()
            .all(|c| c.is_ascii_digit() || matches!(c, '-' | '+' | '.' | 'e' | 'E'));
        if !plain || !s.parse::<f64>().is_ok_and(f64::is_finite) {
            problems.push(format!("{name}: '{v}' is not a number."));
        }
        return s.to_string();
    }
    // PostgreSQL bit(n) / varbit are bit strings
    if dialect == Dialect::Postgres && matches!(b, "bit" | "varbit" | "bit varying") {
        let s = v.trim();
        if !s.chars().all(|c| c == '0' || c == '1') {
            problems.push(format!("{name}: bits are written as 0 and 1, e.g. 1010."));
        }
        return format!("B'{s}'");
    }
    // MySQL BIT(n): shown as a number; 0x... and b'0101' work too
    if dialect == Dialect::MySql && b == "bit" {
        let s = v.trim();
        let lower = s.to_ascii_lowercase();
        if let Some(h) = lower.strip_prefix("0x") {
            if h.is_empty() || !h.chars().all(|c| c.is_ascii_hexdigit()) {
                problems.push(format!("{name}: '{v}' is not a bit value."));
            }
            return format!("X'{h}'");
        }
        if let Some(bits) = lower.strip_prefix("b'").and_then(|r| r.strip_suffix('\'')) {
            if !bits.chars().all(|c| c == '0' || c == '1') {
                problems.push(format!("{name}: '{v}' is not a bit value."));
            }
            return format!("b'{bits}'");
        }
        return match lower.as_str() {
            "true" => "1".into(),
            "false" => "0".into(),
            _ if !s.is_empty() && s.chars().all(|c| c.is_ascii_digit()) => s.to_string(),
            _ => {
                problems.push(format!(
                    "{name}: '{v}' is not a bit value (a number, 0x..., or b'0101')."
                ));
                "0".into()
            }
        };
    }
    if matches!(b, "bit" | "bool" | "boolean") {
        let s = v.trim().to_ascii_lowercase();
        let on = match s.as_str() {
            "1" | "true" | "t" | "yes" => true,
            "0" | "false" | "f" | "no" => false,
            _ => {
                problems.push(format!("{name}: use 1/0 or true/false."));
                false
            }
        };
        return if dialect == Dialect::Postgres {
            if on { "TRUE" } else { "FALSE" }
        } else if on {
            "1"
        } else {
            "0"
        }
        .into();
    }
    if ty.is_binary() {
        // 0x... as shown by SQL Server and MySQL, \x... as shown by PostgreSQL
        let h = v
            .trim()
            .strip_prefix("0x")
            .or_else(|| v.trim().strip_prefix("0X"))
            .or_else(|| v.trim().strip_prefix("\\x"))
            .filter(|h| h.chars().all(|c| c.is_ascii_hexdigit()));
        let Some(h) = h else {
            problems.push(format!("{name}: binary values are written as 0x…"));
            return "NULL".into();
        };
        return match dialect {
            Dialect::Postgres => format!("'\\x{h}'::bytea"),
            Dialect::MySql => format!("X'{h}'"),
            Dialect::MsSql => format!("0x{h}"),
        };
    }
    if ty.is_text()
        && let Some(n) = ty.length()
        && v.chars().count() > n
    {
        problems.push(format!(
            "{name}: {} characters do not fit {}.",
            v.chars().count(),
            col.data_type
        ));
    }
    dialect.typed_string(v, &ty)
}

/// Statements for the edits, with every value checked against its column.
pub fn edit_statements(source: &EditSource, edits: &[RowEdit], dialect: Dialect) -> EditPlan {
    let mut plan = EditPlan::default();
    let q = |s: &str| dialect.quote_ident(s);
    let target = format!("{}.{}", q(&source.schema), q(&source.table));
    let col_of = |name: &str| source.columns.iter().find(|c| c.column.as_deref() == Some(name));
    let keys: Vec<&EditColumn> = source.columns.iter().filter(|c| c.is_key).collect();
    let condition =
        |original: &HashMap<String, Option<String>>, check: &[&String], problems: &mut Vec<String>| -> String {
            let mut parts = Vec::new();
            for k in &keys {
                let n = k.column.as_deref().unwrap_or_default();
                match original.get(n) {
                    Some(Some(v)) => parts.push(format!(
                        "{} = {}",
                        q(n),
                        literal(&Some(v.clone()), k, dialect, problems)
                    )),
                    _ => problems.push(format!("The row has no value for key column {n}.")),
                }
            }
            // optimistic check: the columns being changed still hold what the user saw
            for name in check {
                let Some(c) = col_of(name).filter(|c| !c.is_key && comparable(&c.data_type)) else {
                    continue;
                };
                match original.get(*name) {
                    Some(None) => parts.push(format!("{} IS NULL", q(name))),
                    Some(v) => parts.push(format!("{} = {}", q(name), literal(v, c, dialect, &mut Vec::new()))),
                    None => {}
                }
            }
            parts.join(" AND ")
        };
    for (n, e) in edits.iter().enumerate() {
        let mut problems = Vec::new();
        let stmt = match e {
            RowEdit::Update { original, values } => {
                let mut names: Vec<&String> = values
                    .keys()
                    .filter(|k| col_of(k).is_some_and(|c| !c.read_only))
                    .collect();
                names.sort();
                if names.is_empty() {
                    continue;
                }
                let set = names
                    .iter()
                    .map(|k| {
                        format!(
                            "{} = {}",
                            q(k),
                            literal(&values[*k], col_of(k).expect("checked"), dialect, &mut problems)
                        )
                    })
                    .collect::<Vec<_>>()
                    .join(", ");
                format!(
                    "UPDATE {target} SET {set} WHERE {};",
                    condition(original, &names, &mut problems)
                )
            }
            RowEdit::Insert { values } => {
                // only the columns the user filled in: the others keep their defaults
                let cols: Vec<&EditColumn> = source
                    .columns
                    .iter()
                    .filter(|c| !c.read_only && c.column.as_ref().is_some_and(|n| values.contains_key(n)))
                    .collect();
                if cols.is_empty() {
                    format!("INSERT INTO {target} DEFAULT VALUES;")
                } else {
                    let names: Vec<String> = cols
                        .iter()
                        .map(|c| q(c.column.as_deref().unwrap_or_default()))
                        .collect();
                    let vals: Vec<String> = cols
                        .iter()
                        .map(|c| {
                            literal(
                                &values[c.column.as_deref().unwrap_or_default()],
                                c,
                                dialect,
                                &mut problems,
                            )
                        })
                        .collect();
                    format!(
                        "INSERT INTO {target} ({}) VALUES ({});",
                        names.join(", "),
                        vals.join(", ")
                    )
                }
            }
            RowEdit::Delete { original } => format!(
                "DELETE FROM {target} WHERE {};",
                condition(original, &[], &mut problems)
            ),
        };
        plan.problems
            .extend(problems.into_iter().map(|p| format!("Row {}: {p}", n + 1)));
        plan.statements.push(stmt);
    }
    plan
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::catalog::{Column, DbObject};

    fn catalog() -> Catalog {
        let col = |n: &str, t: &str, nullable: bool, pk: bool, identity: bool| Column {
            name: n.into(),
            data_type: t.into(),
            nullable,
            is_primary_key: pk,
            is_identity: identity,
            is_computed: false,
        };
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
                        col("Id", "int", false, true, true),
                        col("Name", "nvarchar(5)", false, false, false),
                        col("Country", "char(2)", true, false, false),
                        col("Score", "float", true, false, false),
                    ],
                    params: vec![],
                },
                DbObject {
                    schema: "dbo".into(),
                    name: "Logs".into(),
                    kind: ObjectKind::Table,
                    columns: vec![col("Msg", "nvarchar(100)", true, false, false)],
                    params: vec![],
                },
            ],
            foreign_keys: vec![],
        }
    }

    fn names(v: &[&str]) -> Vec<String> {
        v.iter().map(|s| s.to_string()).collect()
    }

    #[test]
    fn decides_what_can_be_edited() {
        let cat = catalog();
        let ms = Dialect::MsSql;
        let s = editable_source(
            "SELECT TOP (200) * FROM dbo.Customers WHERE Id > 1;",
            ms,
            &cat,
            &names(&["Id", "Name", "Country", "Score"]),
        )
        .unwrap();
        assert_eq!(
            s.columns.iter().map(|c| c.column.clone().unwrap()).collect::<Vec<_>>(),
            names(&["Id", "Name", "Country", "Score"])
        );
        assert!(s.columns[0].is_key && s.columns[0].read_only);
        let s = editable_source(
            "SELECT c.Id, c.Name AS N, UPPER(c.Country) AS Country FROM dbo.Customers c",
            ms,
            &cat,
            &names(&["Id", "N", "Country"]),
        )
        .unwrap();
        assert_eq!(s.columns[1].column.as_deref(), Some("Name"));
        assert_eq!(s.columns[2].column, None, "an expression is never written back");
        assert!(
            editable_source("SELECT Name FROM dbo.Customers", ms, &cat, &names(&["Name"]))
                .unwrap_err()
                .contains("primary key column Id")
        );
        assert!(
            editable_source("SELECT * FROM dbo.Logs", ms, &cat, &names(&["Msg"]))
                .unwrap_err()
                .contains("no primary key")
        );
        assert!(
            editable_source(
                "SELECT c.* FROM dbo.Customers c JOIN dbo.Logs l ON 1 = 1",
                ms,
                &cat,
                &[]
            )
            .is_err()
        );
        assert!(
            editable_source(
                "SELECT Country, COUNT(*) FROM dbo.Customers GROUP BY Country",
                ms,
                &cat,
                &[]
            )
            .is_err()
        );
        assert!(editable_source("SELECT 1; SELECT * FROM dbo.Customers", ms, &cat, &[]).is_err());
        assert!(
            editable_source(
                "SELECT Id, Name FROM dbo.Customers WHERE Id IN (SELECT 1)",
                ms,
                &cat,
                &names(&["Id", "Name"])
            )
            .is_ok()
        );
    }

    #[test]
    fn builds_checked_statements() {
        let cat = catalog();
        let src = editable_source(
            "SELECT * FROM dbo.Customers",
            Dialect::MsSql,
            &cat,
            &names(&["Id", "Name", "Country", "Score"]),
        )
        .unwrap();
        let row = |id: &str, name: &str, country: Option<&str>| -> HashMap<String, Option<String>> {
            HashMap::from([
                ("Id".into(), Some(id.into())),
                ("Name".into(), Some(name.into())),
                ("Country".into(), country.map(String::from)),
                ("Score".into(), Some("1.5".into())),
            ])
        };
        let edits = vec![
            RowEdit::Update {
                original: row("1", "An", None),
                values: HashMap::from([
                    ("Country".into(), Some("VN".into())),
                    ("Score".into(), Some("2".into())),
                ]),
            },
            RowEdit::Insert {
                values: HashMap::from([("Name".into(), Some("O'Ne".into()))]),
            },
            RowEdit::Delete {
                original: row("3", "Bo", Some("US")),
            },
        ];
        let plan = edit_statements(&src, &edits, Dialect::MsSql);
        assert_eq!(
            plan.statements,
            vec![
                "UPDATE dbo.Customers SET Country = 'VN', Score = 2 WHERE Id = 1 AND Country IS NULL;",
                "INSERT INTO dbo.Customers (Name) VALUES (N'O''Ne');",
                "DELETE FROM dbo.Customers WHERE Id = 3;",
            ]
        );
        assert!(plan.problems.is_empty(), "{:?}", plan.problems);
        let bad = vec![
            RowEdit::Update {
                original: row("1", "An", None),
                values: HashMap::from([("Name".into(), None)]),
            },
            RowEdit::Update {
                original: row("1", "An", None),
                values: HashMap::from([("Name".into(), Some("toolong".into()))]),
            },
            RowEdit::Update {
                original: row("1", "An", None),
                values: HashMap::from([("Score".into(), Some("abc".into()))]),
            },
        ];
        let plan = edit_statements(&src, &bad, Dialect::MsSql);
        assert_eq!(plan.problems.len(), 3, "{:?}", plan.problems);
        assert!(plan.problems[0].starts_with("Row 1: Name cannot be NULL"));
    }

    #[test]
    fn values_are_written_exactly() {
        let cat = catalog();
        let cols = names(&["Id", "Name", "Country", "Score"]);
        let original = HashMap::from([
            ("Id".to_string(), Some("1".to_string())),
            ("Name".to_string(), Some("An".to_string())),
            ("Country".to_string(), None),
            ("Score".to_string(), None),
        ]);
        let update = |col: &str, value: &str| {
            vec![RowEdit::Update {
                original: original.clone(),
                values: HashMap::from([(col.to_string(), Some(value.to_string()))]),
            }]
        };
        // MySQL reads backslashes in strings as escapes, so they are doubled
        let my = editable_source("SELECT * FROM dbo.Customers", Dialect::MySql, &cat, &cols).unwrap();
        let plan = edit_statements(&my, &update("Name", "a\\b"), Dialect::MySql);
        assert!(
            plan.statements[0].contains("SET Name = 'a\\\\b'"),
            "{:?}",
            plan.statements
        );
        let ms = editable_source("SELECT * FROM dbo.Customers", Dialect::MsSql, &cat, &cols).unwrap();
        let plan = edit_statements(&ms, &update("Name", "a\\b"), Dialect::MsSql);
        assert!(
            plan.statements[0].contains("SET Name = N'a\\b'"),
            "{:?}",
            plan.statements
        );
        // only plain numbers reach the server: NaN, inf and words are refused
        for bad in ["NaN", "inf", "-infinity", "1_000", "0x10", "1e999", ""] {
            let plan = edit_statements(&ms, &update("Score", bad), Dialect::MsSql);
            assert_eq!(plan.problems.len(), 1, "{bad:?} was accepted: {:?}", plan.statements);
        }
        for good in ["0", "-1.5", "+2", "3e2", " 4 "] {
            let plan = edit_statements(&ms, &update("Score", good), Dialect::MsSql);
            assert!(plan.problems.is_empty(), "{good:?}: {:?}", plan.problems);
        }
    }

    #[test]
    fn literals_read_back_as_shown() {
        let source = |types: &[&str]| EditSource {
            schema: "dbo".into(),
            table: "t".into(),
            columns: std::iter::once(EditColumn {
                column: Some("id".into()),
                data_type: "int".into(),
                nullable: false,
                is_key: true,
                read_only: false,
            })
            .chain(types.iter().enumerate().map(|(i, t)| EditColumn {
                column: Some(format!("c{i}")),
                data_type: t.to_string(),
                nullable: true,
                is_key: false,
                read_only: false,
            }))
            .collect(),
        };
        // the SET part of an UPDATE of column c0 (of `ty`) to `value`
        let set = |ty: &str, value: &str, d: Dialect| -> (String, Vec<String>) {
            let plan = edit_statements(
                &source(&[ty]),
                &[RowEdit::Update {
                    original: HashMap::from([("id".to_string(), Some("1".to_string()))]),
                    values: HashMap::from([("c0".to_string(), Some(value.to_string()))]),
                }],
                d,
            );
            let stmt = &plan.statements[0];
            let start = stmt.find(" SET ").unwrap() + 5;
            let end = stmt.find(" WHERE ").unwrap();
            (stmt[start..end].to_string(), plan.problems)
        };
        let (ms, pg, my) = (Dialect::MsSql, Dialect::Postgres, Dialect::MySql);
        // SQL Server: N'...' for every type but 8-bit text, so xml and sysname keep Unicode
        assert_eq!(set("xml", "<a>Việt</a>", ms).0, "c0 = N'<a>Việt</a>'");
        assert_eq!(set("sysname", "Đơn", ms).0, "c0 = N'Đơn'");
        assert_eq!(set("varchar(10)", "abc", ms).0, "c0 = 'abc'");
        // datetime in ISO 8601: 'YYYY-MM-DD hh:mm' depends on DATEFORMAT
        assert_eq!(
            set("datetime", "2024-01-12 10:20:30.123", ms).0,
            "c0 = '2024-01-12T10:20:30.123'"
        );
        assert_eq!(
            set("smalldatetime", "2024-01-12 10:20:00", ms).0,
            "c0 = '2024-01-12T10:20:00'"
        );
        assert_eq!(
            set("datetime2(7)", "2024-01-12 10:20:30.1234567", ms).0,
            "c0 = '2024-01-12 10:20:30.1234567'"
        );
        // MySQL 8 writes unsigned types without a width
        assert_eq!(set("int unsigned", "42", my).0, "c0 = 42");
        assert_eq!(set("int unsigned", "x", my).1.len(), 1);
        assert_eq!(set("varbinary(16)", "0x", ms).0, "c0 = 0x");
        // PostgreSQL: bit strings, bytea as shown (\x...), money in its own format
        assert_eq!(set("bit(8)", "10101010", pg).0, "c0 = B'10101010'");
        assert_eq!(set("bit", "1", pg).0, "c0 = B'1'");
        assert_eq!(set("bit(4)", "12", pg).1.len(), 1);
        assert_eq!(set("boolean", "t", pg).0, "c0 = TRUE");
        assert_eq!(set("bytea", "\\x00ff", pg).0, "c0 = '\\x00ff'::bytea");
        assert_eq!(set("bytea", "0x00FF", pg).0, "c0 = '\\x00FF'::bytea");
        assert_eq!(set("money", "$1,234.56", pg).0, "c0 = '$1,234.56'");
        // MySQL: BIT(n) shown as a number; binary as X'...'
        assert_eq!(set("bit(8)", "170", my).0, "c0 = 170");
        assert_eq!(set("bit(1)", "true", my).0, "c0 = 1");
        assert_eq!(set("bit(8)", "0xAA", my).0, "c0 = X'aa'");
        assert_eq!(set("bit(8)", "b'1010'", my).0, "c0 = b'1010'");
        assert_eq!(set("bit(8)", "x", my).1.len(), 1);
        assert_eq!(set("blob", "0x0102", my).0, "c0 = X'0102'");
        assert_eq!(set("varbinary(4)", "0x", my).0, "c0 = X''");
        assert_eq!(set("blob", "text", my).1.len(), 1);
    }
}
